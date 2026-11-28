// @effect-diagnostics nodeBuiltinImport:off -- Spawns staged Rust binary and legacy Node server against isolated temp git repos for M5 exit verification.
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
  ORCHESTRATION_V2_WS_METHODS,
  OrchestrationV2RpcSchemas,
  ProjectWorkflowBoardSnapshot,
  ServerConfig,
  ServerSettings,
  WorkflowBoardApproveDeliveryResult,
  WorkflowBoardApprovePlanResult,
  WorkflowBoardApproveSpecResult,
  WorkflowBoardArtifact,
  WorkflowBoardChallengeStepResult,
  WorkflowBoardGetCardDiffResult,
  WorkflowBoardPauseCardResult,
  WorkflowBoardRecordManualAcceptanceResult,
  WorkflowBoardResumeCardResult,
  WorkflowBoardRetryStepResult,
  WorkflowBoardStartStageResult,
  WorkflowBoardUpdatePlanResult,
  WorkflowBoardUpdateSpecResult,
  WorkflowCardDetail,
  WorkflowDefinitionFiles,
  WS_METHODS,
} from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Redacted from "effect/Redacted";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import {
  canApproveWorkflowBoardDelivery,
  groupWorkflowBoardCards,
  isProjectWorkflowBoardSupported,
  projectWorkflowBoardAvailability,
  workflowBoardAvailabilityNotice,
  workflowBoardErrorMessage,
  workflowBoardLatestStageRun,
  workflowBoardNeedsRecovery,
  workflowBoardStaleEvidenceSummary,
} from "../../../packages/client-runtime/src/workflowBoard.ts";
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
const decodeServerSettings = Schema.decodeUnknownSync(Schema.toCodecJson(ServerSettings));
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
const decodeArtifact = Schema.decodeUnknownSync(Schema.toCodecJson(WorkflowBoardArtifact));
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
const decodeRecordManualAcceptanceResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardRecordManualAcceptanceResult),
);
const decodeApproveDeliveryResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApproveDeliveryResult),
);
const decodeThreadLaunchResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.launchThread.output),
);
const decodeThreadProjection = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.getThreadProjection.output),
);
const decodeWorkflowDefinitionFiles = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowDefinitionFiles),
);

function resolveRepoRoot(): string {
  return NodePath.resolve(NodePath.dirname(NodeURL.fileURLToPath(import.meta.url)), "../../..");
}

function sha256File(filePath: string): string {
  return NodeCrypto.createHash("sha256").update(NodeFS.readFileSync(filePath)).digest("hex");
}

function newestMtimeMs(dirPath: string): number {
  let latest = 0;
  const stack = [dirPath];
  while (stack.length > 0) {
    const current = stack.pop()!;
    for (const entry of NodeFS.readdirSync(current, { withFileTypes: true })) {
      const fullPath = NodePath.join(current, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === "node_modules" || entry.name === "target" || entry.name === "dist") {
          continue;
        }
        stack.push(fullPath);
      } else if (entry.isFile()) {
        const mtime = NodeFS.statSync(fullPath).mtimeMs;
        if (mtime > latest) {
          latest = mtime;
        }
      }
    }
  }
  return latest;
}

function parseSha256Sums(sumsPath: string): Map<string, string> {
  const entries = new Map<string, string>();
  const raw = NodeFS.readFileSync(sumsPath, "utf8");
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const match = /^([0-9a-f]{64})\s+(.+)$/i.exec(trimmed);
    assert.isNotNull(match, `Malformed SHA256SUMS line: ${trimmed}`);
    entries.set(match![2]!.replaceAll("\\", "/"), match![1]!.toLowerCase());
  }
  return entries;
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
    const shimPath = NodePath.join(dir, `${name}.cmd`);
    NodeFS.writeFileSync(shimPath, `@echo off\r\n"${process.execPath}" "${scriptPath}" %*\r\n`);
    return shimPath;
  }
  const shimPath = NodePath.join(dir, name);
  NodeFS.writeFileSync(shimPath, `#!/bin/sh\nexec "${process.execPath}" "${scriptPath}" "$@"\n`);
  NodeFS.chmodSync(shimPath, 0o755);
  return shimPath;
}

function writeMockReviewTool(tempRoot: string, isWindows: boolean): string {
  const binDir = NodePath.join(tempRoot, "bin");
  NodeFS.mkdirSync(binDir, { recursive: true });
  const reviewScriptPath = NodePath.join(binDir, "mock-review-claude.mjs");
  NodeFS.writeFileSync(
    reviewScriptPath,
    `process.stdin.resume();
const reviewPayload = JSON.stringify({
  verdict: "pass",
  summary: "Independent review passed with zero findings.",
  findings: [],
});
console.log(JSON.stringify({ type: "system", subtype: "init", session_id: "claude-m5-review-1" }));
console.log(JSON.stringify({
  type: "assistant",
  message: {
    id: "msg-m5-review-1",
    role: "assistant",
    content: [{ type: "text", text: reviewPayload }],
    usage: { input_tokens: 18, output_tokens: 12 },
  },
}));
console.log(JSON.stringify({
  type: "result",
  subtype: "success",
  is_error: false,
  usage: { input_tokens: 18, output_tokens: 12 },
}));
process.exit(0);
`,
  );
  return writeExecutableShim(binDir, "mock-review-claude", reviewScriptPath, isWindows);
}

function initIsolatedRepoWithWorktree(tempRoot: string): {
  readonly repoDir: string;
  readonly worktreeDir: string;
} {
  const repoDir = NodePath.join(tempRoot, "repo");
  const worktreeDir = NodePath.join(tempRoot, "repo-wt");
  NodeFS.mkdirSync(repoDir, { recursive: true });
  runGit(["init", "-b", "main"], repoDir);
  runGit(["config", "user.name", "M5 Exit Test"], repoDir);
  runGit(["config", "user.email", "m5-exit@legioncode.local"], repoDir);
  NodeFS.writeFileSync(NodePath.join(repoDir, "README.md"), "# M5 Exit Repo\n", "utf8");
  runGit(["add", "README.md"], repoDir);
  runGit(["commit", "-m", "init m5 repo"], repoDir);
  runGit(["worktree", "add", "-b", "feat/m5-surface", worktreeDir, "main"], repoDir);
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

function startLegacyNodeServer(
  repoRoot: string,
  homeDir: string,
  cwd: string,
): Promise<StartedServer> {
  return new Promise((resolve, reject) => {
    const cleanEnv: NodeJS.ProcessEnv = { ...process.env };
    delete cleanEnv.LEGIONCODE_DEV_AUTH_TOKEN;
    delete cleanEnv.LEGIONCODE_HOME;
    delete cleanEnv.LEGIONCODE_STATE_DIR;
    delete cleanEnv.LEGIONCODE_PORT;
    delete cleanEnv.LEGIONCODE_HOST;
    delete cleanEnv.LEGIONCODE_RUST_BIN;
    delete cleanEnv.VITE_DEV_SERVER_URL;
    cleanEnv.LEGIONCODE_LEGACY_NODE = "1";

    const binTsPath = NodePath.join(repoRoot, "apps", "server", "src", "bin.ts");
    const child = NodeChildProcess.spawn(
      process.execPath,
      [
        binTsPath,
        "serve",
        "--host",
        "127.0.0.1",
        "--base-dir",
        homeDir,
        "--dev-url",
        "http://127.0.0.1:5173",
        cwd,
      ],
      {
        cwd: NodePath.join(repoRoot, "apps", "server"),
        env: cleanEnv,
        stdio: ["pipe", "pipe", "pipe"],
      },
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
            `Legacy Node server exited early (code=${code}, signal=${signal}). stderr:\n${stderrBuf}\nstdout:\n${stdoutBuf}`,
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
        const connMatch = /Connection string:\s*(https?:\/\/\S+)/.exec(line);
        if (connMatch?.[1]) {
          origin = connMatch[1].trim();
        }
        const pairMatch = /Pairing URL:\s*(https?:\/\/\S+)/.exec(line);
        if (pairMatch?.[1]) {
          const parsedUrl = new URL(pairMatch[1].trim());
          origin = parsedUrl.origin;
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

function stopChildServer(child: NodeChildProcess.ChildProcessWithoutNullStreams): Promise<void> {
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
      (f) =>
        f.requestId === requestId &&
        ((f._tag === "Chunk" && Array.isArray(f.values)) || f._tag === "Exit"),
    );
    if (frame._tag === "Exit") {
      throw new Error(`Stream RPC ${requestId} ended unexpectedly: ${JSON.stringify(frame.exit)}`);
    }
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
  return new RpcClient(ws, cookiePair);
}

async function mintPairingCredential(
  origin: string,
  adminCookiePair: string,
  label: string,
): Promise<string> {
  const res = await fetch(`${origin}/api/auth/pairing-token`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: adminCookiePair },
    body: JSON.stringify({ label }),
  });
  assert.strictEqual(res.status, 200);
  const body = (await res.json()) as { readonly credential: string };
  assert.isString(body.credential);
  return body.credential;
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

it.live(
  "verifies staged multi-surface dist parity, SHA256SUMS.txt integrity, freshness, and zero baked dev origins",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const platform = yield* HostProcess.Platform;
      const exeSuffix = platform === "win32" ? ".exe" : "";

      const sumsPath = NodePath.join(repoRoot, "rust", "dist", "SHA256SUMS.txt");
      assert.isTrue(NodeFS.existsSync(sumsPath), "Expected rust/dist/SHA256SUMS.txt");
      const sums = parseSha256Sums(sumsPath);
      assert.isAbove(sums.size, 0);

      const webIndexPath = NodePath.join(repoRoot, "apps", "web", "dist", "index.html");
      const webIndexHtml = NodeFS.readFileSync(webIndexPath, "utf8");
      const webIndexHash = sha256File(webIndexPath);

      const requiredDistEntries = [
        `bin/legioncode${exeSuffix}`,
        `desktop/legioncode-desktop${exeSuffix}`,
        "bin/client/index.html",
        "web-app/index.html",
        "desktop/web-app/index.html",
      ];
      for (const rel of requiredDistEntries) {
        const expectedHash = sums.get(rel);
        assert.isDefined(expectedHash, `Missing ${rel} in rust/dist/SHA256SUMS.txt`);
        const fullPath = NodePath.join(repoRoot, "rust", "dist", rel);
        assert.isTrue(NodeFS.existsSync(fullPath), `Missing staged artifact ${fullPath}`);
        assert.strictEqual(sha256File(fullPath), expectedHash, `SHA-256 mismatch for ${rel}`);
      }

      // Verify exact byte & hash parity across apps/web/dist and all 3 staged web targets.
      for (const stagedIndexRel of [
        "bin/client/index.html",
        "web-app/index.html",
        "desktop/web-app/index.html",
      ]) {
        const stagedPath = NodePath.join(repoRoot, "rust", "dist", stagedIndexRel);
        assert.strictEqual(sha256File(stagedPath), webIndexHash, `${stagedIndexRel} differs`);
        assert.strictEqual(NodeFS.readFileSync(stagedPath, "utf8"), webIndexHtml);
      }

      // Verify referenced /assets/*.js bundles exist identically across all surfaces & match SHA256SUMS.txt.
      const assetRefs = [...webIndexHtml.matchAll(/["'](\/assets\/[^"']+\.js)["']/g)].map(
        (m) => m[1]!,
      );
      assert.isAbove(assetRefs.length, 0, "Expected at least one /assets/*.js script reference");
      for (const ref of assetRefs) {
        const relAsset = ref.replace(/^\/+/, "");
        const webAssetPath = NodePath.join(repoRoot, "apps", "web", "dist", relAsset);
        assert.isTrue(NodeFS.existsSync(webAssetPath), `Missing ${webAssetPath}`);
        const webAssetHash = sha256File(webAssetPath);
        for (const prefix of ["bin/client", "web-app", "desktop/web-app"]) {
          const distRel = `${prefix}/${relAsset}`;
          const stagedAssetPath = NodePath.join(repoRoot, "rust", "dist", distRel);
          assert.isTrue(NodeFS.existsSync(stagedAssetPath), `Missing ${stagedAssetPath}`);
          assert.strictEqual(sha256File(stagedAssetPath), webAssetHash, `Mismatch on ${distRel}`);
          assert.strictEqual(sums.get(distRel), webAssetHash, `SHA256SUMS mismatch on ${distRel}`);
        }
      }

      // Zero baked dev origins or non-empty VITE_HTTP_URL/VITE_WS_URL in apps/web/dist/.
      const forbiddenTokens = [
        "localhost:5733",
        "127.0.0.1:5733",
        "http://localhost:3773",
        "ws://localhost:3773",
        "http://127.0.0.1:3773",
        "ws://127.0.0.1:3773",
        "VITE_HTTP_URL=",
        "VITE_WS_URL=",
      ];
      const nonEmptyViteOriginPattern = /VITE_(?:HTTP|WS)_URL\s*:\s*(?!``|""|'')\S/;
      for (const token of forbiddenTokens) {
        assert.isFalse(
          webIndexHtml.includes(token),
          `Found forbidden token ${token} in index.html`,
        );
      }
      assert.isFalse(
        nonEmptyViteOriginPattern.test(webIndexHtml),
        "Found non-empty VITE_HTTP_URL/VITE_WS_URL in index.html",
      );
      const assetsDir = NodePath.join(repoRoot, "apps", "web", "dist", "assets");
      for (const entry of NodeFS.readdirSync(assetsDir)) {
        if (!entry.endsWith(".js")) continue;
        const jsContent = NodeFS.readFileSync(NodePath.join(assetsDir, entry), "utf8");
        for (const token of forbiddenTokens) {
          assert.isFalse(
            jsContent.includes(token),
            `Found forbidden token ${token} in apps/web/dist/assets/${entry}`,
          );
        }
        assert.isFalse(
          nonEmptyViteOriginPattern.test(jsContent),
          `Found non-empty VITE_HTTP_URL/VITE_WS_URL in apps/web/dist/assets/${entry}`,
        );
      }

      // Source freshness: frontend, shared client, contracts, and Rust crates are not newer than SHA256SUMS.txt.
      const sumsMtime = NodeFS.statSync(sumsPath).mtimeMs;
      for (const sourceRel of [
        "apps/web/src",
        "packages/client-runtime/src",
        "packages/contracts/src",
        "rust/crates",
      ]) {
        const newest = newestMtimeMs(NodePath.join(repoRoot, sourceRel));
        assert.isAtMost(
          newest,
          sumsMtime,
          `Staged rust/dist is older than source changes under ${sourceRel}`,
        );
      }
    }),
);

it.live(
  "completes cross-surface Desktop/Web -> Mobile -> Web journey against one staged legioncode binary",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const platform = yield* HostProcess.Platform;
      const isWindows = platform === "win32";
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env: {} });
      assert.isDefined(binaryPath, "Expected staged rust/dist/bin/legioncode binary");

      const webDistIndexHtml = NodeFS.readFileSync(
        NodePath.join(repoRoot, "apps", "web", "dist", "index.html"),
        "utf8",
      );

      const tempRoot = yield* Effect.acquireRelease(
        Effect.sync(() => NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-m5-exit-"))),
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
      const reviewBinaryPath = writeMockReviewTool(tempRoot, isWindows);

      const host = yield* Effect.acquireRelease(
        Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
        ({ child }) => Effect.promise(() => stopChildServer(child)),
      );

      yield* Effect.promise(async () => {
        // 1. Verify HTTP root bundle parity and projectWorkflowBoard capability.
        const indexRes = await fetch(`${host.origin}/`);
        assert.strictEqual(indexRes.status, 200);
        assert.strictEqual(await indexRes.text(), webDistIndexHtml);

        const envRes = await fetch(`${host.origin}/.well-known/legioncode/environment`);
        assert.strictEqual(envRes.status, 200);
        const descriptor = decodeDescriptor(await envRes.json());
        assert.strictEqual(descriptor.capabilities.projectWorkflowBoard, true);
        assert.isTrue(isProjectWorkflowBoardSupported(descriptor.capabilities));

        const desktopClient = await pairClient(host.origin, host.pairingToken);
        const projectId = "proj-m5-exit";
        let cardId = "";
        let planThreadId = "";
        let mobileFinalCard: WorkflowCardDetail;
        let mobileToken = "";
        let webToken = "";

        try {
          mobileToken = await mintPairingCredential(
            host.origin,
            desktopClient.cookiePair,
            "Mobile Client",
          );
          webToken = await mintPairingCredential(
            host.origin,
            desktopClient.cookiePair,
            "Web Client",
          );
          // 2. Verify Labs-disabled -> enable Labs -> enabled gating via @legioncode/client-runtime.
          const initialConfig = decodeServerConfig(
            await desktopClient.callUnary("cfg-init", "server.getConfig", {}),
          );
          assert.strictEqual(
            projectWorkflowBoardAvailability({
              connected: true,
              configLoaded: true,
              settings: initialConfig.settings,
              capabilities: descriptor.capabilities,
            }),
            "labs-disabled",
          );
          assert.strictEqual(
            workflowBoardAvailabilityNotice("labs-disabled")?.action,
            "open-labs-settings",
          );

          await desktopClient.callUnary("proj", "orchestration.dispatchCommand", {
            type: "project.create",
            commandId: "cmd-proj-m5",
            projectId,
            title: "M5 Cross-Surface Repo",
            workspaceRoot: repoDir,
          });
          const updatedSettings = decodeServerSettings(
            await desktopClient.callUnary("settings", "server.updateSettings", {
              patch: {
                workflowsEnabled: true,
                providerInstances: {
                  "mock-review": {
                    driver: "claudeAgent",
                    enabled: true,
                    config: { binaryPath: reviewBinaryPath },
                  },
                },
              },
            }),
          );
          assert.strictEqual(
            projectWorkflowBoardAvailability({
              connected: true,
              configLoaded: true,
              settings: updatedSettings,
              capabilities: descriptor.capabilities,
            }),
            "enabled",
          );
          assert.isNull(workflowBoardAvailabilityNotice("enabled"));

          desktopClient.sendRequest("sub-desktop", WS_METHODS.workflowBoardSubscribe, {
            projectId,
          });
          const emptySnap = decodeBoardSnapshot(await desktopClient.nextChunk("sub-desktop"));
          assert.strictEqual(emptySnap.cards.length, 0);

          // 3. Desktop/Web client creates card, authors & approves Spec, starts Plan via devReplay, authors & approves Plan.
          const created = decodeCardDetail(
            await desktopClient.callUnary("create-card", WS_METHODS.workflowBoardCreateCard, {
              projectId,
              title: "M5 Multi-Surface Card",
              brief:
                "Deliver across Desktop/Web authoring, Mobile inspection/approval, and Web reopen.",
              sourceType: "codebase",
              intakeGoal: "change",
              assistanceMode: "direct",
              wireframesEnabled: false,
              wireframesIndex: null,
              workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
            }),
          );
          cardId = created.cardId;

          const updatedSpec = decodeUpdateSpecResult(
            await desktopClient.callUnary("upd-spec", WS_METHODS.workflowBoardUpdateSpec, {
              cardId,
              expectedRevision: created.revision,
              acceptanceCriteria: [
                { id: "AC-01", text: "Automated check CHK-01 passes" },
                { id: "AC-02", text: "Mobile operator records manual acceptance" },
              ],
              constraints: ["No auto-commits"],
              sourceRefs: ["README.md"],
            }),
          );

          const challengeDone = decodeChallengeStepResult(
            await desktopClient.callUnary("ch-fallback", WS_METHODS.workflowBoardChallengeStep, {
              cardId,
              expectedRevision: updatedSpec.revision,
              action: "manualFallback",
              acknowledgementToken: "I acknowledge",
              unresolvedConcernIds: [],
              answers: ["Worktree isolated.", "Single task scope.", "Automated check + manual AC."],
            }),
          );

          const specApproved = decodeApproveSpecResult(
            await desktopClient.callUnary("appr-spec", WS_METHODS.workflowBoardApproveSpec, {
              cardId,
              expectedRevision: challengeDone.revision,
            }),
          );
          assert.strictEqual(specApproved.card.stage, "plan");

          const planStarted = decodeStartStageResult(
            await desktopClient.callUnary("start-plan", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: specApproved.revision,
              stage: "plan",
              modelSelection: { instanceId: "devReplay", model: "dev-replay", options: [] },
              runtimeMode: "full-access",
              interactionMode: "default",
              workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
            }),
          );
          planThreadId = planStarted.threadId;
          const planBrief = decodeArtifact(
            await desktopClient.callUnary("get-plan-brief", WS_METHODS.workflowBoardGetArtifact, {
              cardId,
              key: "plan-brief.md",
            }),
          );
          assert.isTrue(planBrief.content.includes("M5 Multi-Surface Card"));

          const planned = decodeUpdatePlanResult(
            await desktopClient.callUnary("upd-plan", WS_METHODS.workflowBoardUpdatePlan, {
              cardId,
              expectedRevision: planStarted.revision,
              lenses: [
                { lens: "ux", applicable: false, reason: "Non-UI feature" },
                { lens: "functional", applicable: true, summary: "Scoped feature change" },
                { lens: "architecture", applicable: true, summary: "Worktree isolated" },
                { lens: "quality", applicable: true, summary: "Check + fresh-thread review" },
                { lens: "infrastructure", applicable: false, reason: "None" },
                { lens: "delivery", applicable: true, summary: "Single task DAG" },
              ],
              alternatives: [
                {
                  id: "alt-1",
                  title: "Single scoped task",
                  summary: "Keep execution in TSK-01",
                  tradeoffs: "Minimal diff",
                },
                {
                  id: "alt-2",
                  title: "Multi-step split",
                  summary: "Split across two tasks",
                  tradeoffs: "Extra overhead",
                },
              ],
              selectedAlternativeId: "alt-1",
              selectionRationale: "Single task keeps changes localized.",
              strategy: "outcomes",
              granularity: "coarse",
              tasks: [
                {
                  taskId: "TSK-01",
                  title: "Execute scoped feature change",
                  filesAllowed: ["README.md"],
                  testingMethod: "existing-checks",
                },
              ],
              checks: [{ id: "CHK-01", argv: ["node", "-e", "process.exit(0)"] }],
              acMappings: [
                { criterionId: "AC-01", method: "automated", checkIds: ["CHK-01"] },
                {
                  criterionId: "AC-02",
                  method: "manual",
                  procedure: "Verify evidence on mobile and record manual acceptance.",
                },
              ],
            }),
          );

          const planApproved = decodeApprovePlanResult(
            await desktopClient.callUnary("appr-plan", WS_METHODS.workflowBoardApprovePlan, {
              cardId,
              expectedRevision: planned.revision,
            }),
          );
          assert.strictEqual(planApproved.card.stage, "execute");

          // Execute TSK-01 via devReplay, run CHK-01, and run fresh-thread review via mock-review.
          const tsk1Started = decodeStartStageResult(
            await desktopClient.callUnary("start-tsk1", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: planApproved.revision,
              stage: "execute",
              taskId: "TSK-01",
              modelSelection: { instanceId: "devReplay", model: "dev-replay", options: [] },
              runtimeMode: "full-access",
            }),
          );
          await waitForBoardSnapshot(desktopClient, "sub-desktop", (snap) => {
            const c = snap.cards.find((entry) => entry.cardId === cardId);
            return c !== undefined && c.revision > tsk1Started.revision;
          });
          const afterTsk1 = decodeCardDetail(
            await desktopClient.callUnary("get-after-tsk1", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(
            afterTsk1.taskExecutions?.find((t) => t.taskId === "TSK-01")?.status,
            "completed",
          );

          const checkResult = decodeRetryStepResult(
            await desktopClient.callUnary("run-chk1", WS_METHODS.workflowBoardRetryStep, {
              cardId,
              expectedRevision: afterTsk1.revision,
              stepKind: "check",
              targetId: "CHK-01",
            }),
          );
          assert.strictEqual(
            checkResult.card.checkReceipts?.find((r) => r.checkId === "CHK-01")?.status,
            "passed",
          );

          const reviewStarted = decodeStartStageResult(
            await desktopClient.callUnary("start-review", WS_METHODS.workflowBoardStartStage, {
              cardId,
              expectedRevision: checkResult.revision,
              stage: "review",
              modelSelection: {
                instanceId: "mock-review",
                model: "claude-sonnet-4-5",
                options: [],
              },
              runtimeMode: "full-access",
            }),
          );
          assert.notStrictEqual(reviewStarted.threadId, planThreadId);
          assert.notStrictEqual(reviewStarted.threadId, tsk1Started.threadId);
          await waitForBoardSnapshot(desktopClient, "sub-desktop", (snap) => {
            const c = snap.cards.find((entry) => entry.cardId === cardId);
            return c !== undefined && c.revision > reviewStarted.revision;
          });
          const afterReview = decodeCardDetail(
            await desktopClient.callUnary("get-after-review", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(afterReview.reviewReport?.verdict, "pass");
          desktopClient.sendInterrupt("sub-desktop");
        } finally {
          desktopClient.close();
        }

        // 4. Pair second client (Mobile) to same legioncode host: inspect, validate selectors, pause/resume, manual AC, ship.
        const mobileClient = await pairClient(host.origin, mobileToken);
        try {
          mobileClient.sendRequest("sub-mobile", WS_METHODS.workflowBoardSubscribe, { projectId });
          const mobileSnap = await waitForBoardSnapshot(mobileClient, "sub-mobile", (snap) =>
            snap.cards.some((c) => c.cardId === cardId),
          );
          const mobileColumns = groupWorkflowBoardCards(mobileSnap.cards);
          assert.strictEqual(mobileColumns.execute.length, 1);
          assert.strictEqual(mobileColumns.execute[0]!.cardId, cardId);

          const mobileDetail = decodeCardDetail(
            await mobileClient.callUnary("mobile-get-card", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          const mobileSpecArtifact = decodeArtifact(
            await mobileClient.callUnary("mobile-spec-art", WS_METHODS.workflowBoardGetArtifact, {
              cardId,
              key: "spec.md",
            }),
          );
          assert.isTrue(mobileSpecArtifact.content.includes("AC-02"));
          const mobilePlanArtifact = decodeArtifact(
            await mobileClient.callUnary("mobile-plan-art", WS_METHODS.workflowBoardGetArtifact, {
              cardId,
              key: "plan.md",
            }),
          );
          assert.isTrue(mobilePlanArtifact.content.includes("TSK-01"));

          const mobileDiff = decodeGetCardDiffResult(
            await mobileClient.callUnary("mobile-diff", WS_METHODS.workflowBoardGetCardDiff, {
              cardId,
              includePatches: true,
            }),
          );
          assert.strictEqual(mobileDiff.cardId, cardId);

          const mobileThread = decodeThreadProjection(
            await mobileClient.callUnary(
              "mobile-thread",
              ORCHESTRATION_V2_WS_METHODS.getThreadProjection,
              {
                threadId: planThreadId,
              },
            ),
          );
          assert.strictEqual(mobileThread.thread.id, planThreadId);

          const staleSummary = workflowBoardStaleEvidenceSummary(mobileDetail);
          assert.strictEqual(staleSummary.totalStale, 0);
          assert.isFalse(workflowBoardNeedsRecovery(mobileDetail));
          assert.strictEqual(
            workflowBoardLatestStageRun(mobileDetail, "plan")?.threadId,
            planThreadId,
          );

          const paused = decodePauseCardResult(
            await mobileClient.callUnary("mobile-pause", WS_METHODS.workflowBoardPauseCard, {
              cardId,
              expectedRevision: mobileDetail.revision,
              reason: "Mobile inspection check",
            }),
          );
          assert.strictEqual(paused.status, "paused");

          const resumed = decodeResumeCardResult(
            await mobileClient.callUnary("mobile-resume", WS_METHODS.workflowBoardResumeCard, {
              cardId,
              expectedRevision: paused.revision,
              reconcileInterrupted: true,
            }),
          );
          assert.strictEqual(resumed.status, "active");

          const snapDigest = resumed.card.shipReadiness?.productSnapshotDigest ?? undefined;
          assert.isString(snapDigest);
          const manualAc = decodeRecordManualAcceptanceResult(
            await mobileClient.callUnary(
              "mobile-manual-ac",
              WS_METHODS.workflowBoardRecordManualAcceptance,
              {
                cardId,
                expectedRevision: resumed.revision,
                criterionId: "AC-02",
                verdict: "pass",
                note: "Verified on mobile",
                evidenceRef: "mobile://m5-exit",
                expectedProductSnapshotDigest: snapDigest,
              },
            ),
          );
          assert.isTrue(
            canApproveWorkflowBoardDelivery({ detail: manualAc.card, canOperate: true }),
          );

          const approvedDelivery = decodeApproveDeliveryResult(
            await mobileClient.callUnary("mobile-ship", WS_METHODS.workflowBoardApproveDelivery, {
              cardId,
              expectedRevision: manualAc.revision,
              expectedProductSnapshotDigest: snapDigest,
              note: "Approved from mobile",
            }),
          );
          assert.strictEqual(approvedDelivery.status, "completed");
          assert.strictEqual(approvedDelivery.deliveryStatus, "shipped");
          assert.strictEqual(approvedDelivery.deliveryApproval.status, "valid");
          mobileFinalCard = approvedDelivery.card;
          mobileClient.sendInterrupt("sub-mobile");
        } finally {
          mobileClient.close();
        }

        // 5. Pair third client (Web) to same host: reopen card and verify exact parity with Mobile + 0 auto-commits.
        const webClient = await pairClient(host.origin, webToken);
        try {
          webClient.sendRequest("sub-web", WS_METHODS.workflowBoardSubscribe, { projectId });
          const webSnap = await waitForBoardSnapshot(webClient, "sub-web", (snap) =>
            snap.cards.some((c) => c.cardId === cardId && c.status === "completed"),
          );
          const webColumns = groupWorkflowBoardCards(webSnap.cards);
          assert.strictEqual(webColumns.spec.length, 0);
          assert.strictEqual(webColumns.plan.length, 0);
          assert.strictEqual(webColumns.execute.length, 0);
          assert.strictEqual(webColumns.ship.length, 0);
          assert.strictEqual(webColumns.completed.length, 1);
          assert.strictEqual(webColumns.completed[0]!.cardId, cardId);

          const webDetail = decodeCardDetail(
            await webClient.callUnary("web-get-card", WS_METHODS.workflowBoardGetCard, {
              cardId,
              projectId,
            }),
          );
          assert.strictEqual(webDetail.revision, mobileFinalCard.revision);
          assert.strictEqual(webDetail.stage, mobileFinalCard.stage);
          assert.strictEqual(webDetail.status, mobileFinalCard.status);
          assert.strictEqual(webDetail.deliveryStatus, mobileFinalCard.deliveryStatus);
          assert.deepStrictEqual(webDetail.deliveryReceipt, mobileFinalCard.deliveryReceipt);
          assert.deepStrictEqual(webDetail.shipReadiness, mobileFinalCard.shipReadiness);
          assert.deepStrictEqual(webDetail.checkReceipts, mobileFinalCard.checkReceipts);
          assert.deepStrictEqual(webDetail.acObservations, mobileFinalCard.acObservations);
          assert.deepStrictEqual(webDetail.stageRuns, mobileFinalCard.stageRuns);
          assert.strictEqual(webDetail.revisions.length, mobileFinalCard.revisions.length);
          assert.strictEqual(runGit(["rev-list", "--count", "HEAD"], repoDir), "1");
          webClient.sendInterrupt("sub-web");
        } finally {
          webClient.close();
        }
      });
    }).pipe(Effect.scoped),
);

it.live(
  "reports unsupported capability cleanly on legacy Node server and pre-board protocol server while preserving chat and /workflows",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const tempRoot = yield* Effect.acquireRelease(
        Effect.sync(() => NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-m5-legacy-"))),
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
      const legacyHomeDir = NodePath.join(tempRoot, "legacy-home");
      NodeFS.mkdirSync(legacyHomeDir, { recursive: true });
      const { repoDir } = initIsolatedRepoWithWorktree(tempRoot);

      // Subcase A: Live Legacy Node Server (`LEGIONCODE_LEGACY_NODE=1` on `apps/server/src/bin.ts`).
      yield* Effect.gen(function* () {
        const legacyHost = yield* Effect.acquireRelease(
          Effect.promise(() => startLegacyNodeServer(repoRoot, legacyHomeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopChildServer(child)),
        );

        yield* Effect.promise(async () => {
          const envRes = await fetch(`${legacyHost.origin}/.well-known/legioncode/environment`);
          assert.strictEqual(envRes.status, 200);
          const descriptor = decodeDescriptor(await envRes.json());
          assert.isUndefined(descriptor.capabilities.projectWorkflowBoard);
          assert.isFalse(isProjectWorkflowBoardSupported(descriptor.capabilities));
          assert.strictEqual(
            projectWorkflowBoardAvailability({
              connected: true,
              configLoaded: true,
              settings: { workflowsEnabled: true },
              capabilities: descriptor.capabilities,
            }),
            "unsupported",
          );
          assert.strictEqual(
            workflowBoardAvailabilityNotice("unsupported")?.title,
            "Workflow board is not supported",
          );

          const client = await pairClient(legacyHost.origin, legacyHost.pairingToken);
          try {
            const getCardCause = await client.callUnaryFailure(
              "legacy-get-card",
              WS_METHODS.workflowBoardGetCard,
              { cardId: "missing-card", projectId: "proj-legacy" },
            );
            const err = extractFailureError(getCardCause);
            assert.strictEqual(err?._tag, "WorkflowBoardRequestError");
            assert.strictEqual(
              workflowBoardErrorMessage(err),
              "This server does not support this workflow board action.",
            );

            // Ordinary chat remains usable (`projects.mutate` project.create + `orchestration.launchThread` with `devReplay`).
            const projectId = "proj-legacy-chat";
            await client.callUnary("legacy-proj", WS_METHODS.projectsMutate, {
              type: "project.create",
              commandId: "cmd-legacy-proj",
              projectId,
              title: "Legacy Chat Project",
              workspaceRoot: repoDir,
            });
            const launched = decodeThreadLaunchResult(
              await client.callUnary("legacy-launch", ORCHESTRATION_V2_WS_METHODS.launchThread, {
                commandId: "cmd-legacy-thread",
                projectId,
                title: "Legacy Ordinary Chat",
                modelSelection: { instanceId: "devReplay", model: "dev-replay", options: [] },
                runtimeMode: "full-access",
                interactionMode: "default",
                workspaceStrategy: { type: "root" },
              }),
            );
            assert.strictEqual(launched.projection.thread.projectId, projectId);

            // `/workflows` inspector RPCs (`workflows.getDefinition` + `workflows.subscribe`) remain usable.
            const defs = decodeWorkflowDefinitionFiles(
              await client.callUnary("legacy-wf-def", WS_METHODS.workflowsGetDefinition, {}),
            );
            assert.isArray(defs.files);
            client.sendRequest("legacy-wf-sub", WS_METHODS.workflowsSubscribe, {});
            client.sendInterrupt("legacy-wf-sub");
          } finally {
            client.close();
          }
        });
      }).pipe(Effect.scoped);

      // Subcase B: Pre-board WS fixture (`projectWorkflowBoard: false` + `UnsupportedRpcMethod`).
      yield* Effect.gen(function* () {
        const fixture = yield* Effect.acquireRelease(
          Effect.promise(
            () =>
              new Promise<{
                readonly origin: string;
                readonly wsOrigin: string;
                readonly close: () => Promise<void>;
              }>((resolve, reject) => {
                const server = NodeHttp.createServer((req, res) => {
                  if (req.url === "/.well-known/legioncode/environment") {
                    res.writeHead(200, { "content-type": "application/json" });
                    res.end(
                      JSON.stringify({
                        capabilities: {
                          repositoryIdentity: true,
                          projectWorkflowBoard: false,
                        },
                      }),
                    );
                    return;
                  }
                  res.writeHead(404);
                  res.end();
                });
                const wss = new NodeWS.WebSocketServer({ server });
                wss.on("connection", (ws) => {
                  ws.on("message", (raw) => {
                    const msg = JSON.parse(raw.toString("utf8")) as {
                      readonly _tag?: string;
                      readonly id?: string;
                      readonly tag?: string;
                    };
                    if (msg._tag !== "Request" || !msg.id) return;
                    if (msg.tag === WS_METHODS.workflowBoardGetCard) {
                      ws.send(
                        JSON.stringify({
                          _tag: "Exit",
                          requestId: msg.id,
                          exit: {
                            _tag: "Failure",
                            cause: [
                              {
                                _tag: "Fail",
                                error: {
                                  _tag: "UnsupportedRpcMethod",
                                  message: "Unsupported RPC method `workflowBoard.getCard`",
                                },
                              },
                            ],
                          },
                        }),
                      );
                      return;
                    }
                    ws.send(
                      JSON.stringify({
                        _tag: "Exit",
                        requestId: msg.id,
                        exit: { _tag: "Success", value: {} },
                      }),
                    );
                  });
                });
                server.once("error", reject);
                server.listen(0, "127.0.0.1", () => {
                  const addr = server.address();
                  if (addr === null || typeof addr === "string") {
                    reject(new Error("Expected numeric port"));
                    return;
                  }
                  resolve({
                    origin: `http://127.0.0.1:${addr.port}`,
                    wsOrigin: `ws://127.0.0.1:${addr.port}`,
                    close: () =>
                      new Promise<void>((res) => {
                        wss.close();
                        server.closeAllConnections();
                        server.close(() => res());
                      }),
                  });
                });
              }),
          ),
          (f) => Effect.promise(() => f.close()),
        );

        yield* Effect.promise(async () => {
          const envRes = await fetch(`${fixture.origin}/.well-known/legioncode/environment`);
          assert.strictEqual(envRes.status, 200);
          const payload = (await envRes.json()) as {
            readonly capabilities: { readonly projectWorkflowBoard?: boolean };
          };
          assert.isFalse(isProjectWorkflowBoardSupported(payload.capabilities));
          assert.strictEqual(
            projectWorkflowBoardAvailability({
              connected: true,
              configLoaded: true,
              settings: { workflowsEnabled: true },
              capabilities: payload.capabilities,
            }),
            "unsupported",
          );

          const ws = new NodeWS.WebSocket(fixture.wsOrigin);
          await new Promise<void>((resolve, reject) => {
            ws.once("open", () => resolve());
            ws.once("error", reject);
          });
          const client = new RpcClient(ws);
          try {
            const cause = await client.callUnaryFailure(
              "preboard-get",
              WS_METHODS.workflowBoardGetCard,
              { cardId: "card-1", projectId: "proj-1" },
            );
            const err = extractFailureError(cause);
            assert.strictEqual(err?._tag, "UnsupportedRpcMethod");
            assert.strictEqual(
              workflowBoardErrorMessage(err),
              "This server does not support this workflow board action.",
            );
            assert.strictEqual(
              workflowBoardErrorMessage("Unknown request tag: workflowBoard.getCard"),
              "This server does not support this workflow board action.",
            );
          } finally {
            client.close();
          }
        });
      }).pipe(Effect.scoped);
    }).pipe(Effect.scoped),
);

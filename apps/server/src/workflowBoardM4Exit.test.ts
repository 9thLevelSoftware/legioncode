// @effect-diagnostics nodeBuiltinImport:off -- Spawns staged Rust binary against isolated temp git repo with disposable .legion-cli copy for M4 exit verification.
import * as NodeChildProcess from "node:child_process";
import * as NodeCrypto from "node:crypto";
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
  ProjectWorkflowBoardSnapshot,
  WorkflowBoardApplyCliImportResult,
  WorkflowBoardPreviewCliImportResult,
  WorkflowBoardRetryStepResult,
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
const decodePreviewCliImport = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardPreviewCliImportResult),
);
const decodeApplyCliImport = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApplyCliImportResult),
);
const decodeRetryStepResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardRetryStepResult),
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

function initDisposableGitRepoWithCliFixture(tempRoot: string, repoRoot: string): string {
  const repoDir = NodePath.join(tempRoot, "repo");
  NodeFS.mkdirSync(repoDir, { recursive: true });
  runGit(["init", "-b", "main"], repoDir);
  runGit(["config", "user.name", "M4 Exit Test"], repoDir);
  runGit(["config", "user.email", "m4-exit@legioncode.local"], repoDir);
  NodeFS.writeFileSync(
    NodePath.join(repoDir, "README.md"),
    "# M4 CLI Import Fixture Repo\n",
    "utf8",
  );
  NodeFS.cpSync(NodePath.join(repoRoot, ".legion-cli"), NodePath.join(repoDir, ".legion-cli"), {
    recursive: true,
  });
  runGit(["add", "."], repoDir);
  runGit(["commit", "-m", "init m4 fixture"], repoDir);
  return repoDir;
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
      client_name: "M4 Exit Agent Token",
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
  "passes M4 exit criteria against staged Rust binary: preview, active-lock refusal, agent guard, explicit idempotent import, policy/blocked-TSK-0002 gates, and restart durability",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveRepoRoot();
      const platform = yield* HostProcess.Platform;
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
        Effect.sync(() => NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-m4-exit-"))),
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
      const repoDir = initDisposableGitRepoWithCliFixture(tempRoot, repoRoot);
      const cliDir = NodePath.join(repoDir, ".legion-cli");
      const projectId = "proj-m4-exit";
      const cardId = "cli-spec-t3-code";

      // Phase 1: Boot Host 1 and verify bundle parity, lock refusals, preview fidelity, agent guard, explicit apply, policy ack, and TSK-0002 retry.
      yield* Effect.gen(function* () {
        const host1 = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopRustServer(child)),
        );

        yield* Effect.promise(async () => {
          const indexRes = await fetch(`${host1.origin}/`);
          assert.strictEqual(indexRes.status, 200);
          assert.strictEqual(await indexRes.text(), stagedIndexHtml);

          const envRes = await fetch(`${host1.origin}/.well-known/legioncode/environment`);
          assert.strictEqual(envRes.status, 200);
          const descriptor = decodeDescriptor(await envRes.json());
          assert.strictEqual(descriptor.capabilities.projectWorkflowBoard, true);

          const client = await pairClient(host1.origin, host1.pairingToken);
          try {
            await client.callUnary("proj", "orchestration.dispatchCommand", {
              type: "project.create",
              commandId: "cmd-proj-m4",
              projectId,
              title: "M4 Exit Repo",
              workspaceRoot: repoDir,
            });
            await client.callUnary("settings", "server.updateSettings", {
              patch: { workflowsEnabled: true },
            });

            client.sendRequest("sub-board", WS_METHODS.workflowBoardSubscribe, { projectId });
            const initialSnap = decodeBoardSnapshot(await client.nextChunk("sub-board"));
            assert.strictEqual(initialSnap.cards.length, 0);

            // 1. Active CLI run locks (engine.lock, live-spawn, resume.json with process.pid) refuse takeover cleanly.
            const lockCases: ReadonlyArray<{
              readonly relPath: string;
              readonly content: string;
              readonly expectedKind: "engine_lock" | "live_spawn" | "resume_json";
            }> = [
              {
                relPath: "index/engine.lock",
                content: JSON.stringify({
                  pid: process.pid,
                  runId: "live-run-1",
                  startedAt: "2026-11-27T00:00:00Z",
                }),
                expectedKind: "engine_lock",
              },
              {
                relPath: "cache/live-spawn/run-live.json",
                content: JSON.stringify({
                  pid: process.pid,
                  runId: "run-live",
                  startedAt: "2026-11-27T00:00:00Z",
                }),
                expectedKind: "live_spawn",
              },
              {
                relPath: "cache/runs/run-active/resume.json",
                content: JSON.stringify({
                  pid: process.pid,
                  runId: "run-active",
                  stage: "execute",
                  startedAt: "2026-11-27T00:00:00Z",
                }),
                expectedKind: "resume_json",
              },
            ];

            for (const [idx, lockCase] of lockCases.entries()) {
              const lockFullPath = NodePath.join(cliDir, lockCase.relPath);
              NodeFS.mkdirSync(NodePath.dirname(lockFullPath), { recursive: true });
              NodeFS.writeFileSync(lockFullPath, lockCase.content, "utf8");

              const lockedPreview = decodePreviewCliImport(
                await client.callUnary(
                  `preview-lock-${idx}`,
                  WS_METHODS.workflowBoardPreviewCliImport,
                  { projectId },
                ),
              );
              assert.isFalse(lockedPreview.canApply);
              assert.isNotNull(lockedPreview.refusalReason);
              assert.isTrue(
                lockedPreview.activeLocks.some(
                  (lock) =>
                    lock.kind === lockCase.expectedKind &&
                    lock.relativePath.includes(lockCase.relPath) &&
                    lock.status === "active",
                ),
              );

              const applyErrCause = await client.callUnaryFailure(
                `apply-lock-${idx}`,
                WS_METHODS.workflowBoardApplyCliImport,
                { projectId },
              );
              const applyErr = extractFailureError(applyErrCause);
              assert.strictEqual(applyErr?._tag, "WorkflowBoardRequestError");
              assert.strictEqual(applyErr?.code, "contract_conflict");
              assert.isFalse(NodeFS.existsSync(NodePath.join(repoDir, ".legion", "workflows")));

              NodeFS.rmSync(lockFullPath, { force: true });
            }

            // Dead-PID lock is surfaced in staleLocks and does NOT block canApply.
            const engineLockPath = NodePath.join(cliDir, "index", "engine.lock");
            NodeFS.mkdirSync(NodePath.dirname(engineLockPath), { recursive: true });
            NodeFS.writeFileSync(
              engineLockPath,
              JSON.stringify({
                pid: 999999,
                runId: "dead-run-1",
                startedAt: "2026-01-01T00:00:00Z",
              }),
              "utf8",
            );

            // Add synthetic workflow/assurance.yaml sidecar in disposable copy only.
            NodeFS.writeFileSync(
              NodePath.join(cliDir, "workflow", "assurance.yaml"),
              "mode: strict\nmaxRetries: 2\n",
              "utf8",
            );

            // 2. Preview real fixture + synthetic assurance sidecar creates 0 cards/files.
            const preview = decodePreviewCliImport(
              await client.callUnary("preview-clean", WS_METHODS.workflowBoardPreviewCliImport, {
                projectId,
              }),
            );
            assert.strictEqual(preview.specId, "spec-t3-code");
            assert.isFalse(preview.alreadyImported);
            assert.isTrue(preview.canApply);
            assert.strictEqual(preview.activeLocks.length, 0);
            assert.isAtLeast(preview.staleLocks.length, 1);
            assert.strictEqual(preview.staleLocks[0]?.status, "stale");
            assert.strictEqual(preview.tasks.length, 8);
            const previewTsk2 = preview.tasks.find((t) => t.taskId === "TSK-0002");
            assert.isDefined(previewTsk2);
            assert.strictEqual(previewTsk2!.status, "blocked");
            assert.deepStrictEqual(previewTsk2!.verificationCommands, [
              ["node", "--test", "packages/provider-mimo/qualification/cli.test.mjs"],
            ]);
            assert.isTrue(
              preview.missingLogNotes.some(
                (note) =>
                  note.taskId === "TSK-0002" &&
                  note.relativePath.includes("verify-1.log") &&
                  note.command === "node --test packages/provider-mimo/qualification/cli.test.mjs",
              ),
            );
            assert.strictEqual(preview.decisionIds.length, 8);
            assert.strictEqual(preview.assumptionIds.length, 18);
            assert.strictEqual(preview.acceptanceCriterionIds.length, 15);
            assert.isAtLeast(preview.historicalReceipts.length, 1);
            for (const receipt of preview.historicalReceipts) {
              assert.strictEqual(receipt.status, "historical");
              assert.isTrue(receipt.requiresNativeRevalidation);
            }
            for (const requiredPolicy of [
              "sandbox.requireHardened",
              "qa.passScore",
              "skills.trustKeys",
              "mcpHttpToolAllowlist",
              "dashboard",
              "workflow.assurance",
            ]) {
              assert.isTrue(
                preview.unsupportedPolicies.some((p) => p.key === requiredPolicy),
                `Missing unsupported policy ${requiredPolicy}`,
              );
            }
            assert.isFalse(NodeFS.existsSync(NodePath.join(repoDir, ".legion", "workflows")));

            // 3. Agent caller guard: applyCliImport refused with EnvironmentAuthorizationError, previewCliImport allowed.
            const agentClient = await connectMcpOAuthAgentClient(host1.origin, client.cookiePair);
            try {
              const agentPreview = decodePreviewCliImport(
                await agentClient.callUnary(
                  "agent-preview",
                  WS_METHODS.workflowBoardPreviewCliImport,
                  { projectId },
                ),
              );
              assert.strictEqual(agentPreview.specId, "spec-t3-code");

              const agentApplyCause = await agentClient.callUnaryFailure(
                "agent-apply",
                WS_METHODS.workflowBoardApplyCliImport,
                { projectId },
              );
              const agentApplyErr = extractFailureError(agentApplyCause);
              assert.strictEqual(agentApplyErr?._tag, "EnvironmentAuthorizationError");
            } finally {
              agentClient.close();
            }

            // 4. Explicit Apply (without acknowledging policies): preserved IDs, deliverables, 0 auto-commits, policy gate.
            const applied1 = decodeApplyCliImport(
              await client.callUnary("apply-1", WS_METHODS.workflowBoardApplyCliImport, {
                projectId,
              }),
            );
            assert.isTrue(applied1.created);
            assert.strictEqual(applied1.cardId, cardId);
            assert.strictEqual(applied1.sourceSpecId, "spec-t3-code");
            assert.strictEqual(applied1.tasks.length, 8);
            assert.strictEqual(
              applied1.tasks.find((t) => t.taskId === "TSK-0002")?.status,
              "blocked",
            );
            assert.strictEqual(applied1.card.cliImport?.owner, "legioncode");
            assert.strictEqual(applied1.card.specApprovalStatus, "historical");
            assert.strictEqual(applied1.card.planApprovalStatus, "historical");
            assert.strictEqual(applied1.card.nextAction, "acknowledge_policies");

            const cardDeliverableDir = NodePath.join(repoDir, ".legion", "workflows", cardId);
            for (const rel of [
              "card.json",
              "import-provenance.json",
              "spec.md",
              "design.md",
              "plan.md",
              "tasks/TSK-0001.md",
              "tasks/TSK-0002.md",
              "tasks/TSK-0008.md",
            ]) {
              assert.isTrue(
                NodeFS.existsSync(NodePath.join(cardDeliverableDir, rel)),
                `Expected deliverable ${rel}`,
              );
            }
            assert.strictEqual(runGit(["rev-list", "--count", "HEAD"], repoDir), "1");

            await waitForBoardSnapshot(
              client,
              "sub-board",
              (snap) => snap.cards.length === 1 && snap.cards[0]?.cardId === cardId,
            );

            // 5. Unacknowledged startStage(execute) fails with invalid_stage_transition.
            const unackedStartCause = await client.callUnaryFailure(
              "start-unacked",
              WS_METHODS.workflowBoardStartStage,
              {
                cardId,
                expectedRevision: applied1.revision,
                stage: "execute",
                modelSelection: { instanceId: "devReplay", model: "dev-replay" },
                runtimeMode: "full-access",
                interactionMode: "default",
                workspaceStrategy: { type: "root" },
              },
            );
            const unackedErr = extractFailureError(unackedStartCause);
            assert.strictEqual(unackedErr?._tag, "WorkflowBoardRequestError");
            assert.strictEqual(unackedErr?.code, "invalid_stage_transition");

            // 6. Idempotent repeat import acknowledging unsupported policies creates 0 duplicates.
            const applied2 = decodeApplyCliImport(
              await client.callUnary("apply-2", WS_METHODS.workflowBoardApplyCliImport, {
                projectId,
                acknowledgeUnsupportedPolicies: true,
              }),
            );
            assert.isFalse(applied2.created);
            assert.strictEqual(applied2.cardId, cardId);
            assert.strictEqual(applied2.tasks.length, 8);
            for (const policy of applied2.unsupportedPolicies) {
              if (policy.blocksExecution) {
                assert.strictEqual(policy.status, "acknowledged");
              }
            }
            const snapAfterRepeat = await waitForBoardSnapshot(
              client,
              "sub-board",
              (snap) => snap.cards.length === 1 && snap.cards[0]?.revision === applied2.revision,
            );
            assert.strictEqual(snapAfterRepeat.cards.length, 1);

            // 7. Blocked TSK-0002 refuses startStage(execute) with contract_conflict; retryStep unblocks TSK-0002.
            const blockedStartCause = await client.callUnaryFailure(
              "start-blocked-tsk2",
              WS_METHODS.workflowBoardStartStage,
              {
                cardId,
                expectedRevision: applied2.revision,
                stage: "execute",
                modelSelection: { instanceId: "devReplay", model: "dev-replay" },
                runtimeMode: "full-access",
                interactionMode: "default",
                workspaceStrategy: { type: "root" },
              },
            );
            const blockedStartErr = extractFailureError(blockedStartCause);
            assert.strictEqual(blockedStartErr?._tag, "WorkflowBoardRequestError");
            assert.strictEqual(blockedStartErr?.code, "contract_conflict");

            const retriedTsk2 = decodeRetryStepResult(
              await client.callUnary("retry-tsk2", WS_METHODS.workflowBoardRetryStep, {
                cardId,
                expectedRevision: applied2.revision,
                stepKind: "task",
                targetId: "TSK-0002",
              }),
            );
            assert.strictEqual(retriedTsk2.stepKind, "task");
            assert.strictEqual(retriedTsk2.targetId, "TSK-0002");
            assert.strictEqual(
              retriedTsk2.card.tasks?.find((t) => t.taskId === "TSK-0002")?.status,
              "todo",
            );

            client.sendInterrupt("sub-board");
          } finally {
            client.close();
          }
        });
      }).pipe(Effect.scoped);

      // Phase 2: Restart Host 2 against same homeDir and verify SQLite durability of imported provenance, 8 tasks, acknowledged policies, and TSK-0002 state.
      yield* Effect.gen(function* () {
        const host2 = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopRustServer(child)),
        );

        yield* Effect.promise(async () => {
          const client2 = await pairClient(host2.origin, host2.pairingToken);
          try {
            const persisted = decodeCardDetail(
              await client2.callUnary("get-persisted", WS_METHODS.workflowBoardGetCard, {
                cardId,
                projectId,
              }),
            );
            assert.strictEqual(persisted.cardId, cardId);
            assert.strictEqual(persisted.cliImport?.sourceSpecId, "spec-t3-code");
            assert.strictEqual(persisted.cliImport?.owner, "legioncode");
            assert.strictEqual(persisted.tasks?.length, 8);
            assert.strictEqual(
              persisted.tasks?.find((t) => t.taskId === "TSK-0002")?.status,
              "todo",
            );
            for (const policy of persisted.cliImport?.unsupportedPolicies ?? []) {
              if (policy.blocksExecution) {
                assert.strictEqual(policy.status, "acknowledged");
              }
            }
          } finally {
            client2.close();
          }
        });
      }).pipe(Effect.scoped);
    }).pipe(Effect.scoped),
);

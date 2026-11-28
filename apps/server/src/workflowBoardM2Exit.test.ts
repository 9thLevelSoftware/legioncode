// @effect-diagnostics nodeBuiltinImport:off -- Spawns staged Rust binary against isolated temp git repo/worktree for M2 exit verification.
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
  WorkflowBoardApprovePlanResult,
  WorkflowBoardApproveSpecResult,
  WorkflowBoardArtifact,
  WorkflowBoardChallengeStepResult,
  WorkflowBoardExploreStepResult,
  WorkflowBoardReturnForRevisionResult,
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
const decodeApproveSpecResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApproveSpecResult),
);
const decodeExploreStepResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardExploreStepResult),
);
const decodeChallengeStepResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardChallengeStepResult),
);
const decodeUpdatePlanResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardUpdatePlanResult),
);
const decodeApprovePlanResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardApprovePlanResult),
);
const decodeReturnForRevisionResult = Schema.decodeUnknownSync(
  Schema.toCodecJson(WorkflowBoardReturnForRevisionResult),
);
const decodeArtifact = Schema.decodeUnknownSync(Schema.toCodecJson(WorkflowBoardArtifact));

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

function initBrownfieldRepoWithWorktree(tempRoot: string): {
  readonly repoDir: string;
  readonly worktreeDir: string;
} {
  const repoDir = NodePath.join(tempRoot, "repo");
  const worktreeDir = NodePath.join(tempRoot, "repo-wt");
  NodeFS.mkdirSync(NodePath.join(repoDir, "src"), { recursive: true });
  runGit(["init", "-b", "main"], repoDir);
  runGit(["config", "user.name", "M2 Exit Test"], repoDir);
  runGit(["config", "user.email", "m2-exit@legioncode.local"], repoDir);
  NodeFS.writeFileSync(
    NodePath.join(repoDir, "package.json"),
    JSON.stringify({ name: "brownfield-fixture", version: "1.0.0" }, null, 2),
    "utf8",
  );
  NodeFS.writeFileSync(NodePath.join(repoDir, "README.md"), "# Brownfield Fixture\n", "utf8");
  NodeFS.writeFileSync(
    NodePath.join(repoDir, "src", "feature.ts"),
    "export const version = 1;\n",
    "utf8",
  );
  runGit(["add", "."], repoDir);
  runGit(["commit", "-m", "init brownfield fixture"], repoDir);
  runGit(["worktree", "add", "-b", "feat/m2-plan", worktreeDir, "main"], repoDir);
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
      client_name: "M2 Exit Agent Token",
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
  "passes M2 exit criteria across brownfield planning, restart persistence, approval invalidation, non-UI wireframes, and staged client parity",
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
        Effect.sync(() => NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-m2-exit-"))),
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
      const { repoDir, worktreeDir } = initBrownfieldRepoWithWorktree(tempRoot);

      const projectId = "proj-m2-exit";
      let uiCardId = "";
      let nonUiCardId = "";
      const operatorExploreAnswer = "Preserve existing public API and add board planning columns";
      const operatorChallengeAnswer =
        "Sandboxed Blob URL uses HtmlRenderDocument without allow-same-origin.";
      const operatorSelectionRationale =
        "Single-pass typed Effect codecs keep Rust and TypeScript in lockstep.";

      // Phase 1: Boot Host 1, verify GET / + capability, complete brownfield UI card from brief to approved plan.
      yield* Effect.gen(function* () {
        const host1 = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopRustServer(child)),
        );

        yield* Effect.promise(async () => {
          const indexRes = await fetch(`${host1.origin}/`);
          assert.strictEqual(indexRes.status, 200);
          const servedIndexHtml = await indexRes.text();
          assert.strictEqual(servedIndexHtml, stagedIndexHtml);

          const envRes = await fetch(`${host1.origin}/.well-known/legioncode/environment`);
          assert.strictEqual(envRes.status, 200);
          const descriptor = decodeDescriptor(await envRes.json());
          assert.strictEqual(descriptor.capabilities.projectWorkflowBoard, true);

          const client1 = await pairClient(host1.origin, host1.pairingToken);
          try {
            await client1.callUnary("proj", "orchestration.dispatchCommand", {
              type: "project.create",
              commandId: "cmd-proj-m2",
              projectId,
              title: "M2 Exit Repo",
              workspaceRoot: repoDir,
            });
            await client1.callUnary("settings", "server.updateSettings", {
              patch: { workflowsEnabled: true },
            });

            client1.sendRequest("sub-board", WS_METHODS.workflowBoardSubscribe, { projectId });
            const initialSnap = decodeBoardSnapshot(await client1.nextChunk("sub-board"));
            assert.strictEqual(initialSnap.cards.length, 0);

            // (a) Brownfield feature with wireframes enabled
            const createdUi = decodeCardDetail(
              await client1.callUnary("create-ui", WS_METHODS.workflowBoardCreateCard, {
                projectId,
                title: "Brownfield Planning Feature",
                brief: "Extend brownfield module with planning workflow.",
                sourceType: "codebase",
                intakeGoal: "change",
                inputPaths: ["package.json", "README.md"],
                assistanceMode: "guided",
                wireframesEnabled: true,
                workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
                modelSelection: { instanceId: "devReplay", model: "dev-replay" },
                runtimeMode: "full-access",
                interactionMode: "default",
              }),
            );
            uiCardId = createdUi.cardId;
            assert.strictEqual(createdUi.spec.sourceType, "codebase");
            assert.strictEqual(createdUi.spec.wireframesIndex, "wireframes/index.html");
            assert.isDefined(createdUi.spec.intakeAnalysis);

            const wfArtifact = decodeArtifact(
              await client1.callUnary("wf-art", WS_METHODS.workflowBoardGetArtifact, {
                cardId: uiCardId,
                key: "wireframes/index.html",
              }),
            );
            assert.isTrue(wfArtifact.content.includes("<html"));
            assert.isTrue(
              NodeFS.existsSync(
                NodePath.join(
                  worktreeDir,
                  ".legion",
                  "workflows",
                  uiCardId,
                  "wireframes",
                  "index.html",
                ),
              ),
            );

            // Resumable guided exploration: seed -> pause -> resume -> edit answers
            const seeded = decodeExploreStepResult(
              await client1.callUnary("exp-seed", WS_METHODS.workflowBoardExploreStep, {
                cardId: uiCardId,
                expectedRevision: createdUi.revision,
                action: "step",
              }),
            );
            assert.isAtLeast(seeded.decisions.length, 1);

            const paused = decodeExploreStepResult(
              await client1.callUnary("exp-pause", WS_METHODS.workflowBoardExploreStep, {
                cardId: uiCardId,
                expectedRevision: seeded.revision,
                action: "pause",
              }),
            );
            assert.isTrue(paused.paused);
            assert.notStrictEqual(paused.card.specApprovalStatus, "valid");

            const resumed = decodeExploreStepResult(
              await client1.callUnary("exp-resume", WS_METHODS.workflowBoardExploreStep, {
                cardId: uiCardId,
                expectedRevision: paused.revision,
                action: "resume",
              }),
            );
            assert.isFalse(resumed.paused);

            const unresolvedIds = resumed.decisions
              .filter((d) => d.status === "unresolved")
              .map((d) => d.id);
            const answeredExplore = decodeExploreStepResult(
              await client1.callUnary("exp-answer", WS_METHODS.workflowBoardExploreStep, {
                cardId: uiCardId,
                expectedRevision: resumed.revision,
                action: "step",
                answers: unresolvedIds.map((decisionId, idx) =>
                  idx === 0
                    ? { decisionId, action: "edit", answer: operatorExploreAnswer }
                    : { decisionId, action: "recommend" },
                ),
              }),
            );
            assert.strictEqual(answeredExplore.exploration.unresolvedIds.length, 0);

            // Update spec with ACs and constraints
            const updatedSpec = decodeUpdateSpecResult(
              await client1.callUnary("upd-spec", WS_METHODS.workflowBoardUpdateSpec, {
                cardId: uiCardId,
                expectedRevision: answeredExplore.revision,
                acceptanceCriteria: [
                  { id: "AC-1", text: "Wireframe renders in sandboxed iframe" },
                  { id: "AC-2", text: "Operator choices persist across host restart" },
                ],
                constraints: ["Never auto-commit artifacts"],
                sourceRefs: ["package.json", "src/feature.ts"],
              }),
            );

            // Bounded challenge: start -> resolve -> synthesize (preserving verbatim answer) -> complete
            const chStart = decodeChallengeStepResult(
              await client1.callUnary("ch-start", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: updatedSpec.revision,
                action: "start",
                concerns: [
                  {
                    id: "CON-1",
                    summary: "Verify wireframe sandbox isolation",
                    whyMatters: "Prevents script access to host session",
                    evidence: "wireframes/index.html",
                    assumption: "Sandboxed iframe lacks allow-same-origin",
                    disposition: "unresolved",
                  },
                ],
              }),
            );
            const chResolve = decodeChallengeStepResult(
              await client1.callUnary("ch-res", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: chStart.revision,
                action: "resolve",
                answers: [
                  {
                    concernId: "CON-1",
                    disposition: "answer",
                    response: operatorChallengeAnswer,
                  },
                ],
              }),
            );
            const chSynth = decodeChallengeStepResult(
              await client1.callUnary("ch-synth", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: chResolve.revision,
                action: "synthesize",
              }),
            );
            assert.strictEqual(chSynth.concerns[0]?.answer, operatorChallengeAnswer);
            const chComplete = decodeChallengeStepResult(
              await client1.callUnary("ch-done", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: chSynth.revision,
                action: "complete",
              }),
            );

            const approvedSpec = decodeApproveSpecResult(
              await client1.callUnary("appr-spec", WS_METHODS.workflowBoardApproveSpec, {
                cardId: uiCardId,
                expectedRevision: chComplete.revision,
              }),
            );
            assert.strictEqual(approvedSpec.specApproval.status, "valid");
            assert.strictEqual(approvedSpec.card.stage, "plan");

            // Author complete Plan (6 lenses, 2 alternatives, strategy, granularity, disjoint DAG, argv checks, 100% AC mapping)
            const updatedPlan = decodeUpdatePlanResult(
              await client1.callUnary("upd-plan", WS_METHODS.workflowBoardUpdatePlan, {
                cardId: uiCardId,
                expectedRevision: approvedSpec.revision,
                lenses: [
                  { lens: "ux", applicable: true, summary: "Sandboxed HTML wireframe preview" },
                  { lens: "functional", applicable: true, summary: "Covers AC-1 and AC-2" },
                  { lens: "architecture", applicable: true, summary: "Rust SQLite + TS codecs" },
                  { lens: "quality", applicable: true, summary: "Automated + manual checks" },
                  { lens: "infrastructure", applicable: false, reason: "Local SQLite only" },
                  { lens: "delivery", applicable: true, summary: "Two sequenced tasks" },
                ],
                alternatives: [
                  {
                    id: "alt-1",
                    title: "Typed lockstep codecs",
                    summary: "Use Effect Schema + Rust contracts end to end.",
                    tradeoffs: "Requires synchronized schema updates.",
                  },
                  {
                    id: "alt-2",
                    title: "Untyped JSON payload",
                    summary: "Pass raw JSON objects.",
                    tradeoffs: "No compile-time wire guarantees.",
                  },
                ],
                selectedAlternativeId: "alt-1",
                selectionRationale: operatorSelectionRationale,
                strategy: "outcomes",
                granularity: "balanced",
                tasks: [
                  {
                    taskId: "TSK-01",
                    title: "Core implementation",
                    filesAllowed: ["src/feature.ts"],
                    filesForbidden: [".env"],
                    verificationCommands: [["git", "status", "--short"]],
                    testingMethod: "test-first",
                  },
                  {
                    taskId: "TSK-02",
                    title: "Docs update",
                    blockedBy: ["TSK-01"],
                    filesAllowed: ["README.md"],
                    testingMethod: "regression-first",
                  },
                ],
                checks: [
                  {
                    id: "CHK-01",
                    description: "Git clean check",
                    argv: ["git", "status", "--short"],
                  },
                ],
                acMappings: [
                  { criterionId: "AC-1", method: "automated", checkIds: ["CHK-01"] },
                  {
                    criterionId: "AC-2",
                    method: "manual",
                    procedure: "Restart host and verify persisted operator answers",
                  },
                ],
              }),
            );
            assert.isNotNull(updatedPlan.planDigest);

            const approvedPlan = decodeApprovePlanResult(
              await client1.callUnary("appr-plan", WS_METHODS.workflowBoardApprovePlan, {
                cardId: uiCardId,
                expectedRevision: updatedPlan.revision,
                expectedSpecDigest: updatedSpec.specDigest ?? undefined,
                expectedPlanDigest: updatedPlan.planDigest ?? undefined,
              }),
            );
            assert.strictEqual(approvedPlan.planApproval.status, "valid");
            assert.strictEqual(approvedPlan.card.stage, "execute");

            client1.sendInterrupt("sub-board");
          } finally {
            client1.close();
          }
        });
      }).pipe(Effect.scoped);

      // Phase 2: Restart Host 2 against same homeDir; verify persisted answers/rationale, scenarios (b), (c), (d).
      yield* Effect.gen(function* () {
        const host2 = yield* Effect.acquireRelease(
          Effect.promise(() => startRustServer(binaryPath!, homeDir, repoDir)),
          ({ child }) => Effect.promise(() => stopRustServer(child)),
        );

        yield* Effect.promise(async () => {
          const client2 = await pairClient(host2.origin, host2.pairingToken);
          try {
            // Verify persistence across server restart
            const persisted = decodeCardDetail(
              await client2.callUnary("get-ui", WS_METHODS.workflowBoardGetCard, {
                cardId: uiCardId,
                projectId,
              }),
            );
            assert.strictEqual(persisted.stage, "execute");
            assert.strictEqual(persisted.specApproval?.status, "valid");
            assert.strictEqual(persisted.planApproval?.status, "valid");
            assert.strictEqual(persisted.selectionRationale, operatorSelectionRationale);
            assert.strictEqual(
              persisted.challengeReceipt?.concerns[0]?.answer,
              operatorChallengeAnswer,
            );
            assert.isTrue(
              persisted.exploration?.decisions.some((d) => d.answer === operatorExploreAnswer),
            );

            // (c) Return to plan / plan-only edit preserves Spec approval while staling Plan approval
            const returnedToPlan = decodeReturnForRevisionResult(
              await client2.callUnary("ret-plan", WS_METHODS.workflowBoardReturnForRevision, {
                cardId: uiCardId,
                expectedRevision: persisted.revision,
                targetStage: "plan",
                reason: "Adjust plan granularity only",
              }),
            );
            assert.strictEqual(returnedToPlan.stage, "plan");
            assert.strictEqual(returnedToPlan.specApproval?.status, "valid");
            assert.strictEqual(returnedToPlan.planApproval?.status, "stale");

            const planOnlyEdit = decodeUpdatePlanResult(
              await client2.callUnary("plan-only", WS_METHODS.workflowBoardUpdatePlan, {
                cardId: uiCardId,
                expectedRevision: returnedToPlan.revision,
                granularity: "fine",
              }),
            );
            assert.strictEqual(planOnlyEdit.specApproval?.status, "valid");
            assert.strictEqual(planOnlyEdit.planApproval?.status, "stale");

            const reapprovedPlan = decodeApprovePlanResult(
              await client2.callUnary("reappr-plan", WS_METHODS.workflowBoardApprovePlan, {
                cardId: uiCardId,
                expectedRevision: planOnlyEdit.revision,
              }),
            );
            assert.strictEqual(reapprovedPlan.planApproval.status, "valid");

            // (b) Editing the spec invalidates BOTH Spec and Plan approvals, regresses stage to spec, and allows full recovery
            const editedSpec = decodeUpdateSpecResult(
              await client2.callUnary("spec-edit-inv", WS_METHODS.workflowBoardUpdateSpec, {
                cardId: uiCardId,
                expectedRevision: reapprovedPlan.revision,
                brief: "Updated spec brief invalidates both Spec and Plan approvals.",
              }),
            );
            assert.strictEqual(editedSpec.specApproval?.status, "stale");
            assert.strictEqual(editedSpec.planApproval?.status, "stale");
            assert.strictEqual(editedSpec.card.stage, "spec");
            assert.strictEqual(editedSpec.card.nextAction, "revise_spec");

            const blockedApprovePlanCause = await client2.callUnaryFailure(
              "approve-plan-blocked",
              WS_METHODS.workflowBoardApprovePlan,
              {
                cardId: uiCardId,
                expectedRevision: editedSpec.revision,
              },
            );
            assert.strictEqual(
              extractFailureError(blockedApprovePlanCause)?.code,
              "spec_not_approved",
            );

            // Stale expectedRevision returns revision_conflict
            const staleRevCause = await client2.callUnaryFailure(
              "spec-edit-stale-rev",
              WS_METHODS.workflowBoardUpdateSpec,
              {
                cardId: uiCardId,
                expectedRevision: 1,
                brief: "Stale revision attempt",
              },
            );
            assert.strictEqual(extractFailureError(staleRevCause)?.code, "revision_conflict");

            // Complete recovery after Spec edit: re-run challenge -> approveSpec -> approvePlan -> execute
            const recChStart = decodeChallengeStepResult(
              await client2.callUnary("rec-ch-start", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: editedSpec.revision,
                action: "start",
                concerns: [
                  {
                    id: "CON-REC-1",
                    summary: "Verify revised spec boundary",
                    whyMatters: "Confirms downstream plan still satisfies revised brief",
                    evidence: "spec.md",
                    assumption: "Plan tasks remain valid",
                    disposition: "unresolved",
                  },
                ],
              }),
            );
            const recChResolve = decodeChallengeStepResult(
              await client2.callUnary("rec-ch-res", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: recChStart.revision,
                action: "resolve",
                answers: [
                  {
                    concernId: "CON-REC-1",
                    disposition: "answer",
                    response: "Plan tasks verified against revised brief.",
                  },
                ],
              }),
            );
            const recChComplete = decodeChallengeStepResult(
              await client2.callUnary("rec-ch-done", WS_METHODS.workflowBoardChallengeStep, {
                cardId: uiCardId,
                expectedRevision: recChResolve.revision,
                action: "complete",
              }),
            );
            const recSpecAppr = decodeApproveSpecResult(
              await client2.callUnary("rec-appr-spec", WS_METHODS.workflowBoardApproveSpec, {
                cardId: uiCardId,
                expectedRevision: recChComplete.revision,
              }),
            );
            assert.strictEqual(recSpecAppr.specApproval.status, "valid");
            assert.strictEqual(recSpecAppr.card.stage, "plan");

            const recPlanAppr = decodeApprovePlanResult(
              await client2.callUnary("rec-appr-plan", WS_METHODS.workflowBoardApprovePlan, {
                cardId: uiCardId,
                expectedRevision: recSpecAppr.revision,
              }),
            );
            assert.strictEqual(recPlanAppr.planApproval.status, "valid");
            assert.strictEqual(recPlanAppr.card.stage, "execute");
            assert.strictEqual(recPlanAppr.card.specApproval?.status, "valid");
            assert.strictEqual(recPlanAppr.card.planApproval?.status, "valid");

            // Agent/MCP OAuth token can read card detail but is rejected with EnvironmentAuthorizationError on human-only approvals/returns
            const agentClient = await connectMcpOAuthAgentClient(host2.origin, client2.cookiePair);
            try {
              const agentRead = decodeCardDetail(
                await agentClient.callUnary("agent-get-card", WS_METHODS.workflowBoardGetCard, {
                  cardId: uiCardId,
                  projectId,
                }),
              );
              assert.strictEqual(agentRead.cardId, uiCardId);

              for (const [reqId, method, payload] of [
                [
                  "agent-appr-spec",
                  WS_METHODS.workflowBoardApproveSpec,
                  { cardId: uiCardId, expectedRevision: recPlanAppr.revision },
                ],
                [
                  "agent-appr-plan",
                  WS_METHODS.workflowBoardApprovePlan,
                  { cardId: uiCardId, expectedRevision: recPlanAppr.revision },
                ],
                [
                  "agent-ret-rev",
                  WS_METHODS.workflowBoardReturnForRevision,
                  {
                    cardId: uiCardId,
                    expectedRevision: recPlanAppr.revision,
                    targetStage: "plan",
                    reason: "Agent must not return card",
                  },
                ],
              ] as const) {
                const authCause = await agentClient.callUnaryFailure(reqId, method, payload);
                assert.strictEqual(
                  extractFailureError(authCause)?._tag,
                  "EnvironmentAuthorizationError",
                );
              }
            } finally {
              agentClient.close();
            }

            // (d) Non-UI feature completes planning cleanly without wireframes (wireframesIndex: null)
            const createdNonUi = decodeCardDetail(
              await client2.callUnary("create-nonui", WS_METHODS.workflowBoardCreateCard, {
                projectId,
                title: "Non-UI Backend Migration",
                brief: "Add index migration without UI.",
                sourceType: "codebase",
                intakeGoal: "change",
                assistanceMode: "direct",
                wireframesEnabled: false,
                wireframesIndex: null,
                workspaceStrategy: { type: "existing_worktree", worktreePath: worktreeDir },
              }),
            );
            nonUiCardId = createdNonUi.cardId;
            assert.isNull(createdNonUi.spec.wireframesIndex);
            assert.isNull(createdNonUi.wireframesIndex ?? null);
            assert.isFalse(
              NodeFS.existsSync(
                NodePath.join(worktreeDir, ".legion", "workflows", nonUiCardId, "wireframes"),
              ),
            );

            await client2.callUnaryFailure("wf-missing", WS_METHODS.workflowBoardGetArtifact, {
              cardId: nonUiCardId,
              key: "wireframes/index.html",
            });

            const nonUiSpec = decodeUpdateSpecResult(
              await client2.callUnary("nonui-spec", WS_METHODS.workflowBoardUpdateSpec, {
                cardId: nonUiCardId,
                expectedRevision: createdNonUi.revision,
                acceptanceCriteria: [{ id: "AC-1", text: "Index created cleanly" }],
                wireframesIndex: null,
              }),
            );
            const nonUiFallback = decodeChallengeStepResult(
              await client2.callUnary("nonui-ch", WS_METHODS.workflowBoardChallengeStep, {
                cardId: nonUiCardId,
                expectedRevision: nonUiSpec.revision,
                action: "manualFallback",
                acknowledgementToken: "I acknowledge",
                unresolvedConcernIds: [],
                answers: [
                  "Migration is idempotent.",
                  "No UI surface affected.",
                  "Verified by SQLite query check.",
                ],
              }),
            );
            assert.strictEqual(nonUiFallback.challengeReceipt.status, "manual_fallback");

            const nonUiSpecAppr = decodeApproveSpecResult(
              await client2.callUnary("nonui-appr-spec", WS_METHODS.workflowBoardApproveSpec, {
                cardId: nonUiCardId,
                expectedRevision: nonUiFallback.revision,
              }),
            );
            const nonUiPlan = decodeUpdatePlanResult(
              await client2.callUnary("nonui-plan", WS_METHODS.workflowBoardUpdatePlan, {
                cardId: nonUiCardId,
                expectedRevision: nonUiSpecAppr.revision,
                lenses: [
                  { lens: "ux", applicable: false, reason: "Backend-only migration" },
                  { lens: "functional", applicable: true, summary: "AC-1" },
                  { lens: "architecture", applicable: true, summary: "Single migration" },
                  { lens: "quality", applicable: true, summary: "Automated check" },
                  { lens: "infrastructure", applicable: false, reason: "Local SQLite" },
                  { lens: "delivery", applicable: true, summary: "One task" },
                ],
                alternatives: [
                  {
                    id: "a1",
                    title: "Online index",
                    summary: "Single statement",
                    tradeoffs: "Fast",
                  },
                  { id: "a2", title: "Rebuild table", summary: "Full copy", tradeoffs: "Slow" },
                ],
                selectedAlternativeId: "a1",
                selectionRationale: "Fast and zero table rebuild.",
                strategy: "outcomes",
                granularity: "coarse",
                tasks: [
                  {
                    taskId: "TSK-01",
                    title: "Add index",
                    filesAllowed: ["src/feature.ts"],
                    testingMethod: "test-first",
                  },
                ],
                checks: [{ id: "CHK-01", argv: ["git", "status", "--short"] }],
                acMappings: [{ criterionId: "AC-1", method: "automated", checkIds: ["CHK-01"] }],
              }),
            );
            const nonUiPlanAppr = decodeApprovePlanResult(
              await client2.callUnary("nonui-appr-plan", WS_METHODS.workflowBoardApprovePlan, {
                cardId: nonUiCardId,
                expectedRevision: nonUiPlan.revision,
              }),
            );
            assert.strictEqual(nonUiPlanAppr.planApproval.status, "valid");
            assert.isNull(nonUiPlanAppr.card.spec.wireframesIndex);
          } finally {
            client2.close();
          }
        });
      }).pipe(Effect.scoped);

      // Zero auto-commits in repository
      assert.strictEqual(runGit(["rev-list", "--count", "HEAD"], repoDir), "1");
    }).pipe(Effect.scoped),
);

// @effect-diagnostics nodeBuiltinImport:off -- E2E tests inspect isolated git worktrees, child processes, and SQLite state.
import * as NodeChildProcess from "node:child_process";
import * as NodeCrypto from "node:crypto";
import * as NodeFS from "node:fs";
import * as NodeHttp from "node:http";
import type * as NodeNet from "node:net";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";
import * as NodeSqlite from "node:sqlite";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, it } from "@effect/vitest";
import { OrchestrationV2RpcSchemas, WorkflowBoardDeliveryReceipt } from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";
import {
  resolveWorkflowBoardRepoRoot,
  startWorkflowBoardE2eHarness,
  WorkflowBoardRpcClient,
  type WorkflowBoardE2eHarness,
} from "./workflowBoardE2eHarness.ts";

const decodeThreadProjection = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.getThreadProjection.output),
);

const HOST_PATH = process.env.PATH ?? process.env.Path ?? "";

interface Counter {
  value: number;
}

function nextId(counter: Counter, prefix: string): string {
  counter.value += 1;
  return `${prefix}-${counter.value}`;
}

function runGit(cwd: string, args: ReadonlyArray<string>): string {
  return NodeChildProcess.execFileSync("git", [...args], {
    cwd,
    env: { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "" },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function initBareRemote(dir: string): string {
  NodeFS.mkdirSync(dir, { recursive: true });
  runGit(dir, ["init", "--bare"]);
  return dir;
}

function runHarnessEffect(
  options: { readonly seedGitWorkspace?: boolean; readonly extraEnv?: NodeJS.ProcessEnv },
  run: (harness: WorkflowBoardE2eHarness) => Promise<void>,
) {
  return Effect.gen(function* () {
    const repoRoot = resolveWorkflowBoardRepoRoot();
    const platform = yield* HostProcess.Platform;
    const env = yield* HostProcess.Environment;
    const binaryPath = resolveRustCliBinary({ repoRoot, platform, env });
    assert.isDefined(binaryPath, "Expected built legioncode binary");
    yield* Effect.promise(async () => {
      await using harness = await startWorkflowBoardE2eHarness({
        repoRoot,
        platform,
        binaryPath,
        seedGitWorkspace: options.seedGitWorkspace ?? true,
        env: options.extraEnv,
      });
      await run(harness);
    });
  });
}

async function probeM3GitDeliveryCapability(harness: WorkflowBoardE2eHarness): Promise<boolean> {
  if (!harness.environment.projectWorkflowBoard) {
    return false;
  }
  const chunks = await runStackedGitAction(harness.rpc, {
    actionId: "m3-git-probe",
    cwd: NodePath.join(harness.tempRoot, "__m3_git_probe__"),
    action: "push",
  });
  return !chunks.some(
    (c) => c.kind === "action_finished" && c.result?.push.status === "skipped_not_requested",
  );
}

async function assertM3ExecuteShipCapability(harness: WorkflowBoardE2eHarness): Promise<void> {
  assert.isTrue(
    harness.environment.projectWorkflowBoard,
    "Expected capabilities.projectWorkflowBoard === true",
  );
  await expectUnaryFailure(
    harness.rpc,
    "workflowBoard.startStage",
    { cardId: "__m3_probe__", expectedRevision: 1, stage: "execute" },
    "card_not_found",
  );
}

async function assertM3GitDeliveryCapability(harness: WorkflowBoardE2eHarness): Promise<void> {
  assert.isTrue(
    harness.environment.projectWorkflowBoard,
    "Expected capabilities.projectWorkflowBoard === true",
  );
  assert.isTrue(
    await probeM3GitDeliveryCapability(harness),
    "Expected M3 git.runStackedAction push/create_pr/filePaths to be implemented",
  );
}

async function createProject(
  rpc: WorkflowBoardRpcClient,
  counter: Counter,
  projectId: string,
  title: string,
  workspaceRoot: string,
): Promise<void> {
  await rpc.callUnary("orchestration.dispatchCommand", {
    type: "project.create",
    commandId: nextId(counter, "cmd-proj"),
    projectId,
    title,
    workspaceRoot,
  });
}

async function awaitThreadRunSettled(
  rpc: WorkflowBoardRpcClient,
  threadId: string,
): Promise<ReturnType<typeof decodeThreadProjection>> {
  const sub = rpc.subscribe("orchestration.subscribeThread", {
    threadId,
    afterSequence: 0,
    requestCompletionMarker: true,
    acceptBoundedSnapshot: true,
    acceptDeferredWork: false,
    acceptCompactTurnItems: false,
    acceptDedupedBodies: false,
    acceptCompactIds: false,
  });

  const isSettled = (proj: ReturnType<typeof decodeThreadProjection>): boolean =>
    proj.runs.some(
      (r) => r.status === "completed" || r.status === "failed" || r.status === "interrupted",
    );

  const readLatest = async (): Promise<ReturnType<typeof decodeThreadProjection>> =>
    decodeThreadProjection(await rpc.callUnary("orchestration.getThreadProjection", { threadId }));

  try {
    const waitLoop = async (): Promise<ReturnType<typeof decodeThreadProjection>> => {
      const current = await readLatest();
      if (isSettled(current)) {
        return current;
      }
      await sub.nextChunk();
      return await waitLoop();
    };
    return await waitLoop();
  } finally {
    await sub.interrupt();
  }
}

interface StackedActionChunk {
  readonly kind?: string;
  readonly result?: {
    readonly action: string;
    readonly commit: { readonly status: string; readonly commitSha?: string };
    readonly push: { readonly status: string };
    readonly pr: { readonly status: string };
  };
  readonly message?: string;
}

async function runStackedGitAction(
  rpc: WorkflowBoardRpcClient,
  payload: Record<string, unknown>,
): Promise<ReadonlyArray<StackedActionChunk>> {
  const sub = rpc.subscribe("git.runStackedAction", payload);
  const chunks: Array<StackedActionChunk> = [];
  try {
    while (true) {
      const chunk = (await sub.nextChunk()) as StackedActionChunk;
      chunks.push(chunk);
      if (chunk.kind === "action_finished" || chunk.kind === "action_failed") {
        break;
      }
    }
    await sub.drainWithProbe();
    return chunks;
  } finally {
    await sub.interrupt();
  }
}

async function expectUnaryFailure(
  rpc: WorkflowBoardRpcClient,
  tag: string,
  payload: unknown,
  expectedFragment: string,
): Promise<string> {
  return await expectUnaryFailureAny(rpc, tag, payload, [expectedFragment]);
}

async function expectUnaryFailureAny(
  rpc: WorkflowBoardRpcClient,
  tag: string,
  payload: unknown,
  expectedFragments: ReadonlyArray<string>,
): Promise<string> {
  try {
    await rpc.callUnary(tag, payload);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    const lower = msg.toLowerCase();
    assert.isTrue(
      expectedFragments.some((frag) => lower.includes(frag.toLowerCase())),
      `Expected RPC "${tag}" error "${msg}" to include one of [${expectedFragments.join(", ")}]`,
    );
    return msg;
  }
  assert.fail(`Expected RPC "${tag}" to fail with one of [${expectedFragments.join(", ")}]`);
}

async function mintAgentWsClient(
  harness: WorkflowBoardE2eHarness,
  threadId: string,
  scopes: ReadonlyArray<string> = ["orchestration:read", "orchestration:operate"],
): Promise<WorkflowBoardRpcClient> {
  const credential = `AGNT-${NodeCrypto.randomUUID()}`;
  const linkId = `link-agent-${NodeCrypto.randomUUID()}`;
  const db = new NodeSqlite.DatabaseSync(harness.dbPath);
  try {
    db.exec("PRAGMA busy_timeout = 5000");
    db.prepare(
      `INSERT INTO auth_pairing_links (
        id, credential, method, scopes, subject, label, created_at, expires_at,
        consumed_at, revoked_at, proof_key_thumbprint
      ) VALUES (?, ?, 'one-time-token', ?, ?, 'agent-e2e', '2020-01-01T00:00:00Z', '2099-01-01T00:00:00Z', NULL, NULL, NULL)`,
    ).run(linkId, credential, JSON.stringify([...scopes]), `thread:${threadId}`);
  } finally {
    db.close();
  }

  const pairRes = await fetch(`${harness.origin}/api/auth/pair`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token: credential, clientLabel: "agent-thread-client" }),
  });
  if (pairRes.status !== 200) {
    throw new Error(`/api/auth/pair for agent token returned ${pairRes.status}`);
  }
  const pairBody = (await pairRes.json()) as { readonly accessToken?: string };
  if (!pairBody.accessToken) {
    throw new Error("Missing accessToken in /api/auth/pair response");
  }
  const wsOrigin = harness.origin.replace(/^http/, "ws");
  const ws = new NodeWS.WebSocket(`${wsOrigin}/ws?orchestrationProtocol=2&frameParts=1`, {
    headers: { authorization: `Bearer ${pairBody.accessToken}` },
  });
  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
  return new WorkflowBoardRpcClient(ws);
}

function createFakeGhBinDir(): { readonly binDir: string; [Symbol.dispose](): void } {
  const binDir = NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "legion-fake-gh-"));
  const jsonOut = JSON.stringify({
    number: 42,
    title: "E2E PR",
    url: "https://github.com/org/repo/pull/42",
    baseRefName: "main",
    headRefName: "feat/f19-pr",
    state: "OPEN",
  });
  NodeFS.writeFileSync(
    NodePath.join(binDir, "gh.cmd"),
    `@echo off\r\necho ${jsonOut.replaceAll('"', '\\"')}\r\n`,
    "utf8",
  );
  const shPath = NodePath.join(binDir, "gh");
  NodeFS.writeFileSync(shPath, `#!/bin/sh\necho '${jsonOut}'\n`, "utf8");
  NodeFS.chmodSync(shPath, 0o755);
  return {
    binDir,
    [Symbol.dispose]: () => {
      NodeFS.rmSync(binDir, { recursive: true, force: true });
    },
  };
}

interface StageStartResult {
  readonly cardId: string;
  readonly revision: number;
  readonly stage: string;
  readonly taskId?: string | null;
  readonly stageRunId: string;
  readonly runId: string | null;
  readonly threadId: string;
  readonly resumed: boolean;
}

interface ShipBlocker {
  readonly id: string;
  readonly kind: string;
  readonly message: string;
  readonly pointer: {
    readonly stage: string;
    readonly tab: string;
    readonly targetId: string | null;
  };
}

interface CheckReceipt {
  readonly checkId: string;
  readonly status: string;
  readonly exitCode: number | null;
  readonly logRef: string | null;
  readonly productSnapshotDigest: string | null;
  readonly executedAt: string | null;
  readonly stderrExcerpt?: string | null;
}

interface ReviewFinding {
  readonly id: string;
  readonly status?: string;
  readonly disposition: string;
}

interface CardDetail {
  readonly cardId: string;
  readonly projectId: string;
  readonly title: string;
  readonly revision: number;
  readonly stage: string;
  readonly status: string;
  readonly stageRuns: ReadonlyArray<{
    readonly id: string;
    readonly status: string;
    readonly threadId: string | null;
  }>;
  readonly plan?: {
    readonly tasks: ReadonlyArray<{
      readonly taskId: string;
      readonly filesAllowed: ReadonlyArray<string>;
    }>;
    readonly checks: ReadonlyArray<{ readonly id: string }>;
  } | null;
  readonly tasks?: ReadonlyArray<{
    readonly taskId: string;
    readonly filesAllowed: ReadonlyArray<string>;
  }>;
  readonly taskExecutions?: ReadonlyArray<{
    readonly taskId: string;
    readonly status: string;
    readonly threadId: string | null;
  }>;
  readonly checkReceipts?: ReadonlyArray<CheckReceipt>;
  readonly fileContractReports?: ReadonlyArray<{
    readonly taskId: string;
    readonly status: string;
    readonly forbiddenFiles: ReadonlyArray<string>;
  }>;
  readonly reviewReport?: {
    readonly status: string;
    readonly verdict?: string;
    readonly threadId: string | null;
    readonly productSnapshotDigest: string | null;
    readonly findings: ReadonlyArray<ReviewFinding>;
  } | null;
  readonly shipReadiness?: { readonly blockers: ReadonlyArray<ShipBlocker> } | null;
}

async function getCard(rpc: WorkflowBoardRpcClient, cardId: string): Promise<CardDetail> {
  return (await rpc.callUnary("workflowBoard.getCard", { cardId })) as CardDetail;
}

const SETTLED_STAGE_RUN_STATUSES = new Set(["completed", "failed", "cancelled"]);

/**
 * Waits for the board turn observer to settle a stage run. The observer persists task/review
 * results asynchronously after the thread run ends, then notifies `workflowBoard.subscribe`,
 * so each board snapshot is the wake-up signal to re-read the card.
 */
async function awaitStageRunSettled(
  rpc: WorkflowBoardRpcClient,
  cardId: string,
  stageRunId: string,
): Promise<CardDetail> {
  const initial = await getCard(rpc, cardId);
  const sub = rpc.subscribe("workflowBoard.subscribe", { projectId: initial.projectId });
  try {
    // The initial snapshot proves the subscription is registered before the re-read below.
    await sub.nextChunk();
    const waitLoop = async (): Promise<CardDetail> => {
      const card = await getCard(rpc, cardId);
      const run = card.stageRuns.find((r) => r.id === stageRunId);
      if (run !== undefined && SETTLED_STAGE_RUN_STATUSES.has(run.status)) {
        return card;
      }
      await sub.nextChunk();
      return await waitLoop();
    };
    return await waitLoop();
  } finally {
    await sub.interrupt();
  }
}

async function startStageSettled(
  rpc: WorkflowBoardRpcClient,
  payload: {
    readonly cardId: string;
    readonly expectedRevision: number;
    readonly stage: string;
    readonly taskId?: string;
    readonly modelSelection?: { readonly instanceId: string; readonly model: string };
  },
): Promise<{ readonly started: StageStartResult; readonly card: CardDetail }> {
  const started = (await rpc.callUnary("workflowBoard.startStage", payload)) as StageStartResult;
  const card = await awaitStageRunSettled(rpc, payload.cardId, started.stageRunId);
  return { started, card };
}

async function retryCheck(
  rpc: WorkflowBoardRpcClient,
  cardId: string,
  expectedRevision: number,
  checkId: string,
): Promise<{
  readonly revision: number;
  readonly stepKind: string;
  readonly targetId: string | null;
  readonly card: CardDetail;
}> {
  return (await rpc.callUnary("workflowBoard.retryStep", {
    cardId,
    expectedRevision,
    stepKind: "check",
    targetId: checkId,
  })) as {
    readonly revision: number;
    readonly stepKind: string;
    readonly targetId: string | null;
    readonly card: CardDetail;
  };
}

const MOCK_CLAUDE_MODEL = "claude-sonnet-4-5";

interface HttpGate {
  readonly port: number;
  readonly entered: Promise<void>;
  release(): void;
  [Symbol.asyncDispose](): Promise<void>;
}

/** Holds a mock provider turn open until the test releases it, so "in progress" is real. */
async function bindHttpGate(): Promise<HttpGate> {
  let markEntered!: () => void;
  const entered = new Promise<void>((resolve) => {
    markEntered = resolve;
  });
  let releaseGate!: () => void;
  const released = new Promise<void>((resolve) => {
    releaseGate = resolve;
  });
  const server = NodeHttp.createServer((_req, res) => {
    markEntered();
    void released.then(() => {
      res.writeHead(200, { "content-type": "text/plain", connection: "close" });
      res.end("ok");
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  const port = (server.address() as NodeNet.AddressInfo).port;
  return {
    port,
    entered,
    release: () => releaseGate(),
    [Symbol.asyncDispose]: async () => {
      releaseGate();
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

function writeClaudeShim(dir: string, name: string, jsBody: string, windows: boolean): string {
  NodeFS.mkdirSync(dir, { recursive: true });
  const scriptPath = NodePath.join(dir, `${name}.mjs`);
  NodeFS.writeFileSync(scriptPath, jsBody, "utf8");
  if (windows) {
    const shimPath = NodePath.join(dir, `${name}.cmd`);
    NodeFS.writeFileSync(shimPath, `@echo off\r\nnode "${scriptPath}" %*\r\n`, "utf8");
    return shimPath;
  }
  const shimPath = NodePath.join(dir, name);
  NodeFS.writeFileSync(shimPath, `#!/bin/sh\nexec node "${scriptPath}" "$@"\n`, "utf8");
  NodeFS.chmodSync(shimPath, 0o755);
  return shimPath;
}

function claudeStreamJs(sessionId: string, msgId: string, textExpr: string, preJs = ""): string {
  return [
    "process.stdin.resume();",
    preJs,
    `console.log(JSON.stringify({ type: 'system', subtype: 'init', session_id: '${sessionId}' }));`,
    `console.log(JSON.stringify({ type: 'assistant', message: { id: '${msgId}', role: 'assistant', content: [{ type: 'text', text: ${textExpr} }], usage: { input_tokens: 10, output_tokens: 8 } } }));`,
    "console.log(JSON.stringify({ type: 'result', subtype: 'success', is_error: false, usage: { input_tokens: 10, output_tokens: 8 } }));",
    "process.exit(0);",
    "",
  ].join("\n");
}

const MOCK_PROVIDER_DIR = NodePath.join(NodeOS.tmpdir(), `legion-e2e-m3-mocks-${process.pid}`);

/**
 * Registers deterministic `claudeAgent` provider instances: structured PASS / findings
 * reviewers, plus an HTTP-gated executor when `gatePort` is given.
 */
async function installMockProviders(
  harness: WorkflowBoardE2eHarness,
  gatePort?: number,
): Promise<void> {
  const windows = harness.environment.descriptor.platform.os === "windows";
  const instances: Record<string, unknown> = {
    "mock-review-pass": {
      driver: "claudeAgent",
      enabled: true,
      config: {
        binaryPath: writeClaudeShim(
          MOCK_PROVIDER_DIR,
          "mock-review-pass",
          claudeStreamJs(
            "claude-rev-pass",
            "msg-rev-pass",
            "JSON.stringify({ verdict: 'pass', summary: 'Clean independent review.', findings: [] })",
          ),
          windows,
        ),
      },
    },
    "mock-review-findings": {
      driver: "claudeAgent",
      enabled: true,
      config: {
        binaryPath: writeClaudeShim(
          MOCK_PROVIDER_DIR,
          "mock-review-findings",
          claudeStreamJs(
            "claude-rev-find",
            "msg-rev-find",
            "JSON.stringify({ verdict: 'findings', summary: 'Found 1 minor issue.', findings: [{ id: 'FND-1', severity: 'minor', criterionId: 'AC-1', file: 'src/ok.ts', line: 1, message: 'Document exported symbol.' }] })",
          ),
          windows,
        ),
      },
    },
  };
  if (gatePort !== undefined) {
    const gatePre = [
      "const http = await import('node:http');",
      "await new Promise((resolve) => {",
      `  const req = http.get('http://127.0.0.1:${gatePort}/wait', (res) => { res.resume(); res.on('end', resolve); });`,
      "  req.on('error', resolve);",
      "});",
    ].join("\n");
    instances["mock-gate"] = {
      driver: "claudeAgent",
      enabled: true,
      config: {
        binaryPath: writeClaudeShim(
          MOCK_PROVIDER_DIR,
          `mock-gate-${gatePort}`,
          claudeStreamJs("claude-gate", "msg-gate", "'Completed gated task.'", gatePre),
          windows,
        ),
      },
    };
  }
  await harness.rpc.callUnary("server.updateSettings", {
    patch: { workflowsEnabled: true, providerInstances: instances },
  });
}

/**
 * Drives an approved-plan card to Ship readiness: every planned task through Execute, every
 * planned check, then an independent structured review (findings when the title carries
 * `[REVIEW_FINDING]`). Returns the settled card revision.
 */
async function runExecuteAndReviewForShip(
  harness: WorkflowBoardE2eHarness,
  cardId: string,
  initialRevision: number,
): Promise<number> {
  const { rpc } = harness;
  await installMockProviders(harness);
  const before = await getCard(rpc, cardId);
  let revision = initialRevision;
  for (const task of before.plan?.tasks ?? []) {
    const { card } = await startStageSettled(rpc, {
      cardId,
      expectedRevision: revision,
      stage: "execute",
      taskId: task.taskId,
    });
    revision = card.revision;
  }
  for (const check of before.plan?.checks ?? []) {
    revision = (await retryCheck(rpc, cardId, revision, check.id)).revision;
  }
  const reviewer = before.title.includes("[REVIEW_FINDING]")
    ? "mock-review-findings"
    : "mock-review-pass";
  const { card } = await startStageSettled(rpc, {
    cardId,
    expectedRevision: revision,
    stage: "review",
    modelSelection: { instanceId: reviewer, model: MOCK_CLAUDE_MODEL },
  });
  return card.revision;
}

interface BoardCardFixture {
  readonly cardId: string;
  readonly revision: number;
  readonly worktreePath: string;
}

const VALID_PLAN_LENSES = [
  { lens: "ux", applicable: true, summary: "Card split layout." },
  { lens: "functional", applicable: true, summary: "Task DAG execution." },
  { lens: "architecture", applicable: true, summary: "Worktree-isolated turns." },
  { lens: "quality", applicable: true, summary: "Planned argv checks and review." },
  { lens: "infrastructure", applicable: false, reason: "Local workspace only." },
  { lens: "delivery", applicable: true, summary: "Discrete commit/push/PR." },
];

const VALID_PLAN_ALTERNATIVES = [
  {
    id: "alt-1",
    title: "Sequential DAG",
    summary: "Separate core and dependent tasks.",
    tradeoffs: "Clear contract isolation.",
  },
  {
    id: "alt-2",
    title: "Single task",
    summary: "Combine changes into one turn.",
    tradeoffs: "Coarser diff.",
  },
];

async function setupApprovedPlanCard(
  rpc: WorkflowBoardRpcClient,
  _counter: Counter,
  projectId: string,
  worktreePath: string,
  options?: {
    readonly cardTitle?: string;
    readonly filesAllowed?: ReadonlyArray<string>;
    readonly filesForbidden?: ReadonlyArray<string>;
    readonly checkCommand?: ReadonlyArray<string>;
    readonly constraints?: ReadonlyArray<string>;
    readonly sourceRefs?: ReadonlyArray<string>;
    readonly tsk2DependsOn?: ReadonlyArray<string>;
  },
): Promise<BoardCardFixture> {
  await rpc.callUnary("server.updateSettings", { patch: { workflowsEnabled: true } });

  const created = (await rpc.callUnary("workflowBoard.createCard", {
    projectId,
    title: options?.cardTitle ?? "Execute/Ship E2E Card",
    workspaceStrategy: { type: "existing_worktree", worktreePath },
    modelSelection: { instanceId: "devReplay", model: "dev-replay" },
    runtimeMode: "auto-accept-edits",
  })) as { readonly cardId: string; readonly revision: number };

  const specUpdated = (await rpc.callUnary("workflowBoard.updateSpec", {
    cardId: created.cardId,
    expectedRevision: created.revision,
    brief: "Deliver scoped feature in isolated worktree",
    acceptanceCriteria: [
      { id: "AC-1", text: "Scoped file change passes planned check" },
      { id: "AC-2", text: "Manual verification recorded before ship" },
    ],
    ...(options?.constraints ? { constraints: options.constraints } : {}),
    ...(options?.sourceRefs ? { sourceRefs: options.sourceRefs } : {}),
  })) as { readonly revision: number };

  const specApproved = (await rpc.callUnary("workflowBoard.approveSpec", {
    cardId: created.cardId,
    expectedRevision: specUpdated.revision,
  })) as { readonly revision: number };

  const filesForbidden = options?.filesForbidden ?? ["src/secret.ts"];
  const planUpdated = (await rpc.callUnary("workflowBoard.updatePlan", {
    cardId: created.cardId,
    expectedRevision: specApproved.revision,
    lenses: VALID_PLAN_LENSES,
    alternatives: VALID_PLAN_ALTERNATIVES,
    selectedAlternativeId: "alt-1",
    selectionRationale: "Two sequential tasks isolate file contracts cleanly.",
    strategy: "outcomes",
    granularity: "balanced",
    tasks: [
      {
        taskId: "TSK-1",
        title: "Primary implementation task",
        dependsOn: [],
        filesAllowed: options?.filesAllowed ?? ["src/ok.ts"],
        filesForbidden,
        testingMethod: "regression-first",
      },
      {
        taskId: "TSK-2",
        title: "Dependent follow-up task",
        dependsOn: options?.tsk2DependsOn ?? ["TSK-1"],
        filesAllowed: ["src/followup.ts"],
        filesForbidden,
        testingMethod: "regression-first",
      },
    ],
    checks: [
      {
        id: "CHK-1",
        criterionId: "AC-1",
        argv: options?.checkCommand ?? ["node", "-e", "process.exit(0)"],
      },
    ],
    acMappings: [
      { criterionId: "AC-1", method: "automated", checkIds: ["CHK-1"] },
      { criterionId: "AC-2", method: "manual", procedure: "Verify manually in worktree." },
    ],
  })) as { readonly revision: number };

  const planApproved = (await rpc.callUnary("workflowBoard.approvePlan", {
    cardId: created.cardId,
    expectedRevision: planUpdated.revision,
  })) as { readonly revision: number };

  return {
    cardId: created.cardId,
    revision: planApproved.revision,
    worktreePath,
  };
}

function connectRawWsStatus(url: string, headers: Record<string, string>): Promise<number> {
  return new Promise((resolve, reject) => {
    const ws = new NodeWS.WebSocket(url, { headers });
    let settled = false;
    ws.once("unexpected-response", (_req, res) => {
      settled = true;
      resolve(res.statusCode ?? 0);
      ws.terminate();
    });
    ws.once("open", () => {
      if (!settled) {
        settled = true;
        ws.close();
        resolve(101);
      }
    });
    ws.once("error", (err) => {
      if (!settled) {
        settled = true;
        reject(err);
      }
    });
  });
}

it.live(
  "[F13-T1-1] Tier 1: orchestration.launchThread binds exact existing_worktree, project, devReplay provider, and runtimeMode",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f13-wt");
      await createProject(harness.rpc, counter, "proj-f13", "F13 Project", harness.workspaceDir);

      const launched = (await harness.rpc.callUnary("orchestration.launchThread", {
        commandId: nextId(counter, "cmd-launch"),
        creationSource: "web",
        projectId: "proj-f13",
        title: "F13 Existing Worktree Launch",
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        runtimeMode: "auto-accept-edits",
        interactionMode: "default",
        workspaceStrategy: {
          type: "existing_worktree",
          worktreePath: wtPath,
          branch: "feat/f13-wt",
        },
        initialMessage: {
          messageId: nextId(counter, "msg"),
          text: "Execute F13 worktree launch",
          attachments: [],
        },
      })) as {
        readonly threadId: string;
        readonly resumed: boolean;
        readonly projection: {
          readonly thread: {
            readonly projectId: string;
            readonly worktreePath: string | null;
            readonly branch: string | null;
            readonly runtimeMode: string;
          };
        };
      };

      assert.isFalse(launched.resumed);
      assert.strictEqual(launched.projection.thread.projectId, "proj-f13");
      assert.strictEqual(launched.projection.thread.worktreePath, wtPath);
      assert.strictEqual(launched.projection.thread.branch, "feat/f13-wt");
      assert.strictEqual(launched.projection.thread.runtimeMode, "auto-accept-edits");

      const settled = await awaitThreadRunSettled(harness.rpc, launched.threadId);
      assert.isTrue(settled.runs.some((r) => r.status === "completed"));
    }),
);

it.live(
  "[F13-T1-2] Tier 1: orchestration.launchThread with workspaceStrategy=worktree creates isolated worktree and settles turn",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const counter: Counter = { value: 0 };
      await createProject(harness.rpc, counter, "proj-f13-wt", "F13 WT", harness.workspaceDir);

      const launched = (await harness.rpc.callUnary("orchestration.launchThread", {
        commandId: nextId(counter, "cmd-wt-launch"),
        projectId: "proj-f13-wt",
        title: "Worktree Isolation Launch",
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        runtimeMode: "approval-required",
        interactionMode: "plan",
        workspaceStrategy: {
          type: "worktree",
          baseRef: "HEAD",
          branch: "feat/f13-auto-wt",
        },
        initialMessage: {
          messageId: nextId(counter, "msg-wt"),
          text: "Plan isolated worktree feature",
          attachments: [],
        },
      })) as {
        readonly threadId: string;
        readonly projection: {
          readonly thread: {
            readonly projectId: string;
            readonly worktreePath: string | null;
            readonly branch: string | null;
            readonly runtimeMode: string;
          };
        };
      };

      assert.strictEqual(launched.projection.thread.projectId, "proj-f13-wt");
      assert.isString(launched.projection.thread.worktreePath);
      assert.notStrictEqual(launched.projection.thread.worktreePath, harness.workspaceDir);
      assert.isTrue(NodeFS.existsSync(launched.projection.thread.worktreePath!));
      assert.strictEqual(launched.projection.thread.branch, "feat/f13-auto-wt");
      assert.strictEqual(launched.projection.thread.runtimeMode, "approval-required");

      const settled = await awaitThreadRunSettled(harness.rpc, launched.threadId);
      assert.isTrue(settled.runs.some((r) => r.status === "completed"));
    }),
);

it.live(
  "[F13-T1-3] Tier 1 (Gated): workflowBoard.startStage and stableLaunchKey recover single thread across restart()",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f13-board");
      await createProject(
        harness.rpc,
        counter,
        "proj-f13-board",
        "F13 Board",
        harness.workspaceDir,
      );

      const stableLaunchKey = "card-f13:plan:attempt-1";
      const first = (await harness.rpc.callUnary("orchestration.launchThread", {
        commandId: nextId(counter, "cmd-idem"),
        projectId: "proj-f13-board",
        title: "Idempotent Stage Thread",
        stableLaunchKey,
        reuseExistingThread: true,
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        runtimeMode: "approval-required",
        interactionMode: "plan",
        workspaceStrategy: {
          type: "existing_worktree",
          worktreePath: wtPath,
          branch: "feat/f13-board",
        },
      })) as { readonly threadId: string; readonly resumed: boolean };
      assert.isFalse(first.resumed);

      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f13-board", wtPath);
      await using gate = await bindHttpGate();
      await installMockProviders(harness, gate.port);
      // The gated provider keeps TSK-1 in flight across the restart.
      const started1 = (await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
        modelSelection: { instanceId: "mock-gate", model: MOCK_CLAUDE_MODEL },
      })) as StageStartResult;
      await gate.entered;

      await harness.restart();

      const second = (await harness.rpc.callUnary("orchestration.launchThread", {
        commandId: nextId(counter, "cmd-idem-retry"),
        projectId: "proj-f13-board",
        title: "Idempotent Stage Thread",
        stableLaunchKey,
        reuseExistingThread: true,
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        runtimeMode: "approval-required",
        interactionMode: "plan",
        workspaceStrategy: {
          type: "existing_worktree",
          worktreePath: wtPath,
          branch: "feat/f13-board",
        },
      })) as { readonly threadId: string; readonly resumed: boolean };
      assert.isTrue(second.resumed);
      assert.strictEqual(second.threadId, first.threadId);

      const started2 = (await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: card.cardId,
        expectedRevision: started1.revision,
        stage: "execute",
      })) as StageStartResult;
      assert.strictEqual(started2.threadId, started1.threadId);
      assert.isTrue(started2.resumed);

      const proj = decodeThreadProjection(
        await harness.rpc.callUnary("orchestration.getThreadProjection", {
          threadId: started1.threadId,
        }),
      );
      assert.strictEqual(proj.thread.projectId, "proj-f13-board");
      assert.strictEqual(proj.thread.worktreePath, wtPath);
      assert.strictEqual(proj.thread.runtimeMode, "auto-accept-edits");
      gate.release();
    }),
);

it.live(
  "[F13-T2-1] Tier 2: missing or unknown projectId fails with OrchestrationV2ThreadLaunchError without first-project fallback",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const counter: Counter = { value: 0 };
      await createProject(harness.rpc, counter, "proj-active-only", "Active", harness.workspaceDir);

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-missing-proj"),
          projectId: "proj-does-not-exist",
          title: "Should fail",
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: { type: "root" },
        },
        "OrchestrationV2ThreadLaunchError",
      );

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-blank-proj"),
          projectId: "   ",
          title: "Should fail blank",
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: { type: "root" },
        },
        "ParseError",
      );

      const db = harness.openReadOnlyDb();
      try {
        const row = db
          .prepare("SELECT COUNT(*) AS cnt FROM orchestration_v2_projection_threads")
          .get() as { readonly cnt: number };
        assert.strictEqual(row.cnt, 0);
      } finally {
        db.close();
      }
    }),
);

it.live(
  "[F13-T2-2] Tier 2: empty model, invalid runtimeMode, and invalid workspaceStrategy reject without silent default/FullAccess fallback",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const counter: Counter = { value: 0 };
      await createProject(harness.rpc, counter, "proj-f13-strict", "Strict", harness.workspaceDir);

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-empty-model"),
          projectId: "proj-f13-strict",
          title: "Empty Model",
          modelSelection: { instanceId: "devReplay", model: "" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: { type: "root" },
        },
        "ParseError",
      );

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-bad-mode"),
          projectId: "proj-f13-strict",
          title: "Bad Mode",
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "bogus-mode",
          interactionMode: "default",
          workspaceStrategy: { type: "root" },
        },
        "ParseError",
      );

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-bad-ws"),
          projectId: "proj-f13-strict",
          title: "Bad Strategy",
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: { type: "bogus_strategy" },
        },
        "ParseError",
      );

      const db = harness.openReadOnlyDb();
      try {
        const row = db
          .prepare("SELECT COUNT(*) AS cnt FROM orchestration_v2_projection_threads")
          .get() as { readonly cnt: number };
        assert.strictEqual(row.cnt, 0);
      } finally {
        db.close();
      }
    }),
);

it.live(
  "[F13-T2-3] Tier 2 (Gated): missing existing_worktree and stableLaunchKey reuse conflict reject with OrchestrationV2ThreadLaunchError",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      assert.isTrue(harness.environment.projectWorkflowBoard);
      const counter: Counter = { value: 0 };
      const wtA = harness.git!.createWorktree("feat/f13-conflict-a");
      const wtB = harness.git!.createWorktree("feat/f13-conflict-b");
      await createProject(harness.rpc, counter, "proj-f13-a", "Proj A", harness.workspaceDir);
      await createProject(harness.rpc, counter, "proj-f13-b", "Proj B", wtB);

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-missing-wt"),
          projectId: "proj-f13-a",
          title: "Missing Worktree",
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: {
            type: "existing_worktree",
            worktreePath: NodePath.join(harness.tempRoot, "no-such-worktree"),
          },
        },
        "OrchestrationV2ThreadLaunchError",
      );

      const original = (await harness.rpc.callUnary("orchestration.launchThread", {
        commandId: nextId(counter, "cmd-orig"),
        projectId: "proj-f13-a",
        title: "Original Launch",
        stableLaunchKey: "key-reuse-conflict",
        reuseExistingThread: true,
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        runtimeMode: "approval-required",
        interactionMode: "default",
        workspaceStrategy: {
          type: "existing_worktree",
          worktreePath: wtA,
          branch: "feat/f13-conflict-a",
        },
      })) as { readonly threadId: string };

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-mismatch-proj"),
          projectId: "proj-f13-b",
          title: "Mismatch Project",
          stableLaunchKey: "key-reuse-conflict",
          reuseExistingThread: true,
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: { type: "existing_worktree", worktreePath: wtA },
        },
        "OrchestrationV2ThreadLaunchError",
      );

      await expectUnaryFailure(
        harness.rpc,
        "orchestration.launchThread",
        {
          commandId: nextId(counter, "cmd-mismatch-wt"),
          projectId: "proj-f13-a",
          title: "Mismatch Worktree",
          stableLaunchKey: "key-reuse-conflict",
          reuseExistingThread: true,
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "default",
          workspaceStrategy: { type: "existing_worktree", worktreePath: wtB },
        },
        "OrchestrationV2ThreadLaunchError",
      );

      const proj = decodeThreadProjection(
        await harness.rpc.callUnary("orchestration.getThreadProjection", {
          threadId: original.threadId,
        }),
      );
      assert.strictEqual(proj.thread.projectId, "proj-f13-a");
      assert.strictEqual(proj.thread.worktreePath, wtA);
    }),
);

it.live(
  "[F14-T1-1] Tier 1 (Gated): Execute selects dependency-ready tasks and blocks dependent TSK-2 until TSK-1 completes",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f14-dag");
      await createProject(harness.rpc, counter, "proj-f14", "F14 Project", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14", wtPath);

      const before = await getCard(harness.rpc, card.cardId);
      assert.deepStrictEqual(
        before.plan?.tasks.map((t) => t.taskId),
        ["TSK-1", "TSK-2"],
      );
      const statusOf = (detail: CardDetail, taskId: string) =>
        detail.taskExecutions?.find((t) => t.taskId === taskId)?.status;
      assert.strictEqual(statusOf(before, "TSK-1"), "todo");
      assert.strictEqual(statusOf(before, "TSK-2"), "todo");

      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.startStage",
        { cardId: card.cardId, expectedRevision: card.revision, stage: "execute", taskId: "TSK-2" },
        ["dependency", "WorkflowBoardRequestError"],
      );

      const { card: detail } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      assert.strictEqual(statusOf(detail, "TSK-1"), "completed");
      assert.strictEqual(statusOf(detail, "TSK-2"), "todo");

      const { started: started2, card: afterTsk2 } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: detail.revision,
        stage: "execute",
        taskId: "TSK-2",
      });
      assert.strictEqual(started2.taskId, "TSK-2");
      assert.strictEqual(statusOf(afterTsk2, "TSK-2"), "completed");
    }),
);

it.live(
  "[F14-T1-2] Tier 1 (Gated): single mutating task concurrency cap rejects concurrent task on the same card",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f14-single");
      await createProject(harness.rpc, counter, "proj-f14-cap", "F14 Cap", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14-cap", wtPath, {
        tsk2DependsOn: [],
      });
      await using gate = await bindHttpGate();
      await installMockProviders(harness, gate.port);

      await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
        modelSelection: { instanceId: "mock-gate", model: MOCK_CLAUDE_MODEL },
      });
      await gate.entered;
      const mid = await getCard(harness.rpc, card.cardId);
      assert.strictEqual(
        mid.taskExecutions?.find((t) => t.taskId === "TSK-1")?.status,
        "in_progress",
      );

      const errMsg = await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.startStage",
        { cardId: card.cardId, expectedRevision: mid.revision, stage: "execute", taskId: "TSK-2" },
        ["WorkflowBoardRequestError", "active", "mutating", "task"],
      );
      assert.notInclude(errMsg.toLowerCase(), "revision_conflict");
      gate.release();
    }),
);

it.live(
  "[F14-T1-3] Tier 1 (Gated): post-turn diff validation approves filesAllowed and preserves unexpected edits on disk",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f14-contract");
      await createProject(harness.rpc, counter, "proj-f14-diff", "F14 Diff", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14-diff", wtPath, {
        filesAllowed: ["src/ok.ts"],
      });

      const okAbs = NodePath.join(wtPath, "src", "ok.ts");
      const unexpectedAbs = NodePath.join(wtPath, "src", "unexpected.ts");
      NodeFS.mkdirSync(NodePath.dirname(unexpectedAbs), { recursive: true });
      NodeFS.writeFileSync(okAbs, "export const ok = 1;\n", "utf8");
      NodeFS.writeFileSync(unexpectedAbs, "export const preserved = true;\n", "utf8");

      const { card: detail } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });

      assert.isTrue(NodeFS.existsSync(unexpectedAbs));
      assert.strictEqual(
        NodeFS.readFileSync(unexpectedAbs, "utf8"),
        "export const preserved = true;\n",
      );
      assert.isFalse(
        (detail.shipReadiness?.blockers ?? []).some((b) => b.kind === "contract_violation"),
      );
      assert.strictEqual(
        detail.taskExecutions?.find((t) => t.taskId === "TSK-1")?.status,
        "completed",
      );
      const diff = (await harness.rpc.callUnary("workflowBoard.getCardDiff", {
        cardId: card.cardId,
        taskId: "TSK-1",
      })) as {
        readonly files: ReadonlyArray<{
          readonly path: string;
          readonly contractStatus?: string | null;
        }>;
      };
      assert.strictEqual(diff.files.find((f) => f.path === "src/ok.ts")?.contractStatus, "allowed");
      assert.strictEqual(
        diff.files.find((f) => f.path === "src/unexpected.ts")?.contractStatus,
        "unexpected",
      );
    }),
);

it.live(
  "[F14-T2-1] Tier 2: filesForbidden violation blocks continuation and preserves the forbidden edit",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f14-forbidden");
      await createProject(harness.rpc, counter, "proj-f14-forb", "F14 Forb", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14-forb", wtPath, {
        filesAllowed: ["src/**"],
        filesForbidden: ["src/secret.ts"],
      });

      const forbiddenAbs = NodePath.join(wtPath, "src", "secret.ts");
      await using gate = await bindHttpGate();
      await installMockProviders(harness, gate.port);
      const started = await harness.rpc.callUnary<StageStartResult>("workflowBoard.startStage", {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
        modelSelection: { instanceId: "mock-gate", model: MOCK_CLAUDE_MODEL },
      });
      await gate.entered;
      NodeFS.mkdirSync(NodePath.dirname(forbiddenAbs), { recursive: true });
      NodeFS.writeFileSync(forbiddenAbs, "SECRET=1\n", "utf8");
      gate.release();

      const detail = await awaitStageRunSettled(harness.rpc, card.cardId, started.stageRunId);
      const report = detail.fileContractReports?.find((item) => item.taskId === "TSK-1");
      assert.strictEqual(report?.status, "violation_pending");
      assert.include(report?.forbiddenFiles ?? [], "src/secret.ts");
      assert.isTrue(
        detail.shipReadiness?.blockers.some((blocker) => blocker.kind === "contract_violation"),
      );
      assert.isTrue(NodeFS.existsSync(forbiddenAbs));
    }),
);

it.live(
  "[F14-T2-2] Tier 2 (Gated): empty diff / zero-file task completion boundary handles deterministic status without crash",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f14-empty");
      await createProject(
        harness.rpc,
        counter,
        "proj-f14-empty",
        "F14 Empty",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14-empty", wtPath);

      const { card: detail } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });

      const diff = (await harness.rpc.callUnary("workflowBoard.getCardDiff", {
        cardId: card.cardId,
      })) as { readonly files: ReadonlyArray<unknown> };
      assert.isArray(diff.files);
      assert.strictEqual(diff.files.length, 0);
      assert.isFalse(
        detail.shipReadiness?.blockers.some((blocker) => blocker.kind === "contract_violation"),
      );
      assert.strictEqual(
        detail.taskExecutions?.find((task) => task.taskId === "TSK-1")?.status,
        "completed",
      );
    }),
);

it.live(
  "[F14-T2-3] Tier 2 (Gated): multiple cards in separate worktrees execute concurrently without cross-card blocking",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtA = harness.git!.createWorktree("feat/f14-card-a");
      const wtB = harness.git!.createWorktree("feat/f14-card-b");
      await createProject(
        harness.rpc,
        counter,
        "proj-f14-multi",
        "F14 Multi",
        harness.workspaceDir,
      );
      const cardA = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14-multi", wtA, {
        cardTitle: "Card A",
      });
      const cardB = await setupApprovedPlanCard(harness.rpc, counter, "proj-f14-multi", wtB, {
        cardTitle: "Card B",
      });

      const startA = (await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: cardA.cardId,
        expectedRevision: cardA.revision,
        stage: "execute",
        taskId: "TSK-1",
      })) as { readonly threadId: string };
      const startB = (await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: cardB.cardId,
        expectedRevision: cardB.revision,
        stage: "execute",
        taskId: "TSK-1",
      })) as { readonly threadId: string };

      assert.notStrictEqual(startA.threadId, startB.threadId);
      await awaitThreadRunSettled(harness.rpc, startA.threadId);
      await awaitThreadRunSettled(harness.rpc, startB.threadId);
    }),
);

it.live(
  "[F15-T1-1] Tier 1 (Gated): pauseCard and resumeCard preserve completed steps and persist across restart()",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f15-pause");
      await createProject(harness.rpc, counter, "proj-f15", "F15 Project", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f15", wtPath);

      const { card: prePause } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });

      const paused = (await harness.rpc.callUnary("workflowBoard.pauseCard", {
        cardId: card.cardId,
        expectedRevision: prePause.revision,
      })) as { readonly status: string; readonly revision: number };
      assert.strictEqual(paused.status, "paused");

      await harness.restart();

      const afterRestart = await getCard(harness.rpc, card.cardId);
      assert.strictEqual(afterRestart.status, "paused");

      const resumed = (await harness.rpc.callUnary("workflowBoard.resumeCard", {
        cardId: card.cardId,
        expectedRevision: paused.revision,
      })) as { readonly status: string };
      assert.notStrictEqual(resumed.status, "paused");

      const postResume = await getCard(harness.rpc, card.cardId);
      const completedTsk1 = (postResume.taskExecutions ?? []).filter(
        (t) => t.taskId === "TSK-1" && t.status === "completed",
      );
      assert.strictEqual(completedTsk1.length, 1);
    }),
);

it.live(
  "[F15-T1-2] Tier 1 (Gated): retryStep increments attempt counter and clears blocker once underlying issue is fixed",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f15-retry");
      await createProject(
        harness.rpc,
        counter,
        "proj-f15-retry",
        "F15 Retry",
        harness.workspaceDir,
      );
      const markerPath = NodePath.join(wtPath, "ready.txt");
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f15-retry", wtPath, {
        checkCommand: [
          "node",
          "-e",
          "process.exit(require('node:fs').existsSync('ready.txt') ? 0 : 1)",
        ],
      });

      const { card: executed } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      const firstRun = await retryCheck(harness.rpc, card.cardId, executed.revision, "CHK-1");

      const preRetry = await getCard(harness.rpc, card.cardId);
      const failedReceipt = preRetry.checkReceipts?.find((r) => r.checkId === "CHK-1");
      assert.strictEqual(failedReceipt?.status, "failed");
      assert.isTrue(
        (preRetry.shipReadiness?.blockers ?? []).some((b) => b.kind === "check_failed"),
      );

      NodeFS.writeFileSync(markerPath, "ok\n", "utf8");
      const retried = await retryCheck(harness.rpc, card.cardId, preRetry.revision, "CHK-1");
      assert.strictEqual(retried.stepKind, "check");
      assert.strictEqual(retried.targetId, "CHK-1");
      assert.isTrue(retried.revision > firstRun.revision);

      const postRetry = await getCard(harness.rpc, card.cardId);
      const passedReceipt = postRetry.checkReceipts?.find((r) => r.checkId === "CHK-1");
      assert.strictEqual(passedReceipt?.status, "passed");
      assert.isFalse(
        (postRetry.shipReadiness?.blockers ?? []).some((b) => b.kind === "check_failed"),
      );
    }),
);

it.live(
  "[F15-T1-3] Tier 1 (Gated): proposeAmendment widens scope via human approval and marks affected evidence stale",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f15-amend");
      await createProject(
        harness.rpc,
        counter,
        "proj-f15-amend",
        "F15 Amend",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f15-amend", wtPath);

      const { card: postExec } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      const checked = await retryCheck(harness.rpc, card.cardId, postExec.revision, "CHK-1");
      assert.strictEqual(
        checked.card.checkReceipts?.find((r) => r.checkId === "CHK-1")?.status,
        "passed",
      );

      const amended = (await harness.rpc.callUnary("workflowBoard.proposeAmendment", {
        cardId: card.cardId,
        expectedRevision: checked.revision,
        targetStage: "plan",
        reason: "Include src/extra.ts in TSK-1 contract",
        tasks: [
          {
            taskId: "TSK-1",
            title: "Primary implementation task",
            dependsOn: [],
            filesAllowed: ["src/ok.ts", "src/extra.ts"],
            filesForbidden: ["src/secret.ts"],
            testingMethod: "regression-first",
          },
          {
            taskId: "TSK-2",
            title: "Dependent follow-up task",
            dependsOn: ["TSK-1"],
            filesAllowed: ["src/followup.ts"],
            filesForbidden: ["src/secret.ts"],
            testingMethod: "regression-first",
          },
        ],
      })) as { readonly revision: number };

      const approved = (await harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: card.cardId,
        expectedRevision: amended.revision,
      })) as { readonly revision: number };
      assert.isTrue(approved.revision > amended.revision);

      const detail = await getCard(harness.rpc, card.cardId);
      const tsk1 = detail.plan?.tasks.find((t) => t.taskId === "TSK-1");
      assert.isDefined(tsk1);
      assert.include(tsk1!.filesAllowed, "src/extra.ts");
      assert.strictEqual(detail.checkReceipts?.find((r) => r.checkId === "CHK-1")?.status, "stale");
    }),
);

it.live(
  "[F15-T2-1] Tier 2 (Gated): stale expectedRevision rejects pauseCard, resumeCard, retryStep, and proposeAmendment without mutation",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f15-stale-rev");
      await createProject(harness.rpc, counter, "proj-f15-rev", "F15 Rev", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f15-rev", wtPath);

      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.pauseCard",
        { cardId: card.cardId, expectedRevision: card.revision - 1 },
        ["revision_conflict"],
      );
      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.resumeCard",
        { cardId: card.cardId, expectedRevision: card.revision - 1 },
        ["revision_conflict"],
      );
      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.retryStep",
        {
          cardId: card.cardId,
          expectedRevision: card.revision - 1,
          stepKind: "check",
          targetId: "CHK-1",
        },
        ["revision_conflict"],
      );
      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.proposeAmendment",
        {
          cardId: card.cardId,
          expectedRevision: card.revision - 1,
          reason: "stale rev",
          patch: {},
        },
        ["revision_conflict"],
      );
      const unchanged = (await harness.rpc.callUnary("workflowBoard.getCard", {
        cardId: card.cardId,
      })) as { readonly revision: number; readonly status: string };
      assert.strictEqual(unchanged.revision, card.revision);
      assert.notStrictEqual(unchanged.status, "paused");
    }),
);

it.live(
  "[F15-T2-2] Tier 2 (Gated): resumeCard reconciles interrupted external action before continuing",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f15-reconcile");
      await createProject(harness.rpc, counter, "proj-f15-rec", "F15 Rec", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f15-rec", wtPath);

      const paused = (await harness.rpc.callUnary("workflowBoard.pauseCard", {
        cardId: card.cardId,
        expectedRevision: card.revision,
      })) as { readonly revision: number };

      const db = new NodeSqlite.DatabaseSync(harness.dbPath);
      try {
        db.prepare(
          `INSERT INTO workflow_board_external_actions
             (id, card_id, action_kind, idempotency_key, status, created_at, updated_at)
           VALUES ('f15-interrupted', ?, 'planned_check', 'f15-interrupted', 'interrupted',
                   '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z')`,
        ).run(card.cardId);
      } finally {
        db.close();
      }
      const resumed = (await harness.rpc.callUnary("workflowBoard.resumeCard", {
        cardId: card.cardId,
        expectedRevision: paused.revision,
        reconcileInterrupted: true,
      })) as {
        readonly status: string;
        readonly reconciledCount: number;
      };
      assert.notStrictEqual(resumed.status, "paused");
      assert.strictEqual(resumed.reconciledCount, 1);
    }),
);

it.live(
  "[F15-T2-3] Tier 2 (Gated): invalid state transition boundaries (double pause / retry non-failed step) reject cleanly",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f15-invalid-trans");
      await createProject(harness.rpc, counter, "proj-f15-inv", "F15 Inv", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f15-inv", wtPath);

      const retryMsg = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.retryStep",
        {
          cardId: card.cardId,
          expectedRevision: card.revision,
          stepKind: "task",
          targetId: "TSK-1",
        },
        "must be `failed` or `blocked`",
      );
      assert.include(retryMsg.toLowerCase(), "failed");
    }),
);

it.live(
  "[F16-T1-1] Tier 1 (Gated): argv-only planned check runs in card worktree and binds receipt to product snapshot",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f16-pass");
      await createProject(harness.rpc, counter, "proj-f16", "F16 Project", harness.workspaceDir);
      // Passes only when the check's cwd is a linked worktree, whose `.git` is a file.
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f16", wtPath, {
        checkCommand: [
          "node",
          "-e",
          "process.exit(require('node:fs').statSync('.git').isFile() ? 0 : 1)",
        ],
      });

      const { card: executed } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      await retryCheck(harness.rpc, card.cardId, executed.revision, "CHK-1");

      const detail = await getCard(harness.rpc, card.cardId);
      const rec = detail.checkReceipts?.find((r) => r.checkId === "CHK-1");
      assert.isDefined(rec);
      assert.strictEqual(rec?.status, "passed");
      assert.strictEqual(rec?.exitCode, 0);
      assert.isTrue((rec?.productSnapshotDigest?.length ?? 0) > 0);
    }),
);

it.live(
  "[F16-T1-2] Tier 1 (Gated): failed planned check blocks card in Execute with command, exit code, and host log pointer",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f16-fail");
      await createProject(harness.rpc, counter, "proj-f16-fail", "F16 Fail", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f16-fail", wtPath, {
        checkCommand: ["node", "-e", "process.exit(console.error('boom') ?? 1)"],
      });

      const { card: executed } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      await retryCheck(harness.rpc, card.cardId, executed.revision, "CHK-1");

      const detail = await getCard(harness.rpc, card.cardId);
      const blocker = detail.shipReadiness?.blockers.find((b) => b.kind === "check_failed");
      assert.isDefined(blocker);
      assert.strictEqual(blocker?.pointer.stage, "execute");
      assert.strictEqual(blocker?.pointer.targetId, "CHK-1");
      const rec = detail.checkReceipts?.find((r) => r.checkId === "CHK-1");
      assert.strictEqual(rec?.status, "failed");
      assert.strictEqual(rec?.exitCode, 1);
      assert.isTrue((rec?.logRef?.length ?? 0) > 0);
    }),
);

it.live(
  "[F16-T1-3] Tier 1 (Gated): product snapshot change automatically marks prior passing check receipt stale",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f16-stale");
      await createProject(
        harness.rpc,
        counter,
        "proj-f16-stale",
        "F16 Stale",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f16-stale", wtPath, {
        checkCommand: ["node", "-e", "process.exit(0)"],
      });

      const { card: executed } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      const checked = await retryCheck(harness.rpc, card.cardId, executed.revision, "CHK-1");
      assert.strictEqual(
        checked.card.checkReceipts?.find((r) => r.checkId === "CHK-1")?.status,
        "passed",
      );

      NodeFS.writeFileSync(NodePath.join(wtPath, "README.md"), "# mutated snapshot\n", "utf8");
      const detail = await getCard(harness.rpc, card.cardId);
      assert.strictEqual(detail.checkReceipts?.find((r) => r.checkId === "CHK-1")?.status, "stale");
    }),
);

it.live(
  "[F16-T2-1] Tier 2 (Gated): planned check child environment scrubs sensitive server credentials",
  () =>
    runHarnessEffect(
      {
        seedGitWorkspace: true,
        extraEnv: { OPENAI_API_KEY: "sk-secret-e2e-probe", ANTHROPIC_API_KEY: "ant-secret-e2e" },
      },
      async (harness) => {
        await assertM3ExecuteShipCapability(harness);
        const counter: Counter = { value: 0 };
        const wtPath = harness.git!.createWorktree("feat/f16-scrub");
        await createProject(
          harness.rpc,
          counter,
          "proj-f16-scrub",
          "F16 Scrub",
          harness.workspaceDir,
        );
        const script = NodePath.join(wtPath, "check-env.cjs");
        NodeFS.writeFileSync(
          script,
          "process.exit(process.env.OPENAI_API_KEY || process.env.ANTHROPIC_API_KEY ? 2 : 0)\n",
        );
        const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f16-scrub", wtPath, {
          checkCommand: [process.execPath, script],
        });
        const checked = await retryCheck(harness.rpc, card.cardId, card.revision, "CHK-1");
        assert.strictEqual(
          checked.card.checkReceipts?.find((receipt) => receipt.checkId === "CHK-1")?.status,
          "passed",
        );
      },
    ),
);

it.live(
  "[F16-T2-2] Tier 2 (Gated): shell control operators are rejected before the planned check can run",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f16-argv");
      await createProject(harness.rpc, counter, "proj-f16-argv", "F16 Argv", harness.workspaceDir);
      const sentinel = NodePath.join(wtPath, "pwned.txt");
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f16-argv", wtPath);
      const parseError = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.updatePlan",
        {
          cardId: card.cardId,
          expectedRevision: card.revision,
          checks: [{ id: "CHK-1", criterionId: "AC-1", argv: ["node", "-e", "; echo pwned"] }],
        },
        "ParseError",
      );
      assert.include(parseError, "argv-only command without shell wrappers or control operators");
      assert.isFalse(NodeFS.existsSync(sentinel));
      assert.strictEqual((await getCard(harness.rpc, card.cardId)).revision, card.revision);
    }),
);

it.live(
  "[F16-T2-3] Tier 2 (Gated): nonexistent executable in planned check records structured failed receipt without server crash",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f16-missing-bin");
      await createProject(
        harness.rpc,
        counter,
        "proj-f16-nobin",
        "F16 NoBin",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f16-nobin", wtPath, {
        checkCommand: ["definitely-missing-legion-check-binary-xyz"],
      });

      const launchError = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.retryStep",
        {
          cardId: card.cardId,
          expectedRevision: card.revision,
          stepKind: "check",
          targetId: "CHK-1",
        },
        "launch_failed",
      );
      assert.include(launchError, "definitely-missing-legion-check-binary-xyz");
      const checked = await getCard(harness.rpc, card.cardId);

      const probe = (await harness.rpc.callUnary("server.probe", {})) as {
        readonly serverTime: string;
      };
      assert.isString(probe.serverTime);
      const receipt = checked.checkReceipts?.find((entry) => entry.checkId === "CHK-1");
      assert.strictEqual(receipt?.status, "failed");
      assert.strictEqual(receipt?.exitCode, null);
      assert.match(receipt?.productSnapshotDigest ?? "", /^sha256:[0-9a-f]{64}$/);
      assert.include(receipt?.stderrExcerpt ?? "", "definitely-missing-legion-check-binary-xyz");
      assert.isString(receipt?.logRef);
    }),
);

it.live(
  "[F17-T1-1] Tier 1 (Gated): independent review launches in a fresh thread distinct from Execute task threads",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f17-fresh");
      await createProject(harness.rpc, counter, "proj-f17", "F17 Project", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f17", wtPath);
      await installMockProviders(harness);

      const { started: execStart, card: postExec } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });

      const { started: revStart, card: postReview } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: postExec.revision,
        stage: "review",
        modelSelection: { instanceId: "mock-review-pass", model: MOCK_CLAUDE_MODEL },
      });
      assert.notStrictEqual(revStart.threadId, execStart.threadId);
      assert.isFalse(
        (postReview.taskExecutions ?? []).some((t) => t.threadId === revStart.threadId),
      );
      assert.strictEqual(postReview.reviewReport?.threadId, revStart.threadId);
    }),
);

it.live(
  "[F17-T1-2] Tier 1 (Gated): structured PASS review records snapshot-bound review receipt",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f17-pass");
      await createProject(harness.rpc, counter, "proj-f17-pass", "F17 Pass", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f17-pass", wtPath);
      await installMockProviders(harness);

      const { card: detail } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "review",
        modelSelection: { instanceId: "mock-review-pass", model: MOCK_CLAUDE_MODEL },
      });
      assert.strictEqual(detail.reviewReport?.verdict, "pass");
      assert.strictEqual(detail.reviewReport?.status, "completed");
      assert.isTrue((detail.reviewReport?.productSnapshotDigest?.length ?? 0) > 0);
    }),
);

it.live(
  "[F17-T1-3] Tier 1 (Gated): review findings block Ship until human resolves via workflowBoard.resolveFinding",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f17-findings");
      await createProject(harness.rpc, counter, "proj-f17-find", "F17 Find", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f17-find", wtPath, {
        cardTitle: "Review Finding Card [REVIEW_FINDING]",
      });
      await installMockProviders(harness);

      const { card: postExec } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      const { card: detail } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: postExec.revision,
        stage: "review",
        modelSelection: { instanceId: "mock-review-findings", model: MOCK_CLAUDE_MODEL },
      });

      const findings = detail.reviewReport?.findings ?? [];
      assert.isTrue(findings.length > 0);
      assert.strictEqual(findings[0]!.status, "open");
      assert.strictEqual(findings[0]!.disposition, "unresolved");

      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.approveDelivery",
        { cardId: card.cardId, expectedRevision: detail.revision },
        ["WorkflowBoardRequestError", "finding"],
      );

      const resolved = (await harness.rpc.callUnary("workflowBoard.resolveFinding", {
        cardId: card.cardId,
        expectedRevision: detail.revision,
        findingId: findings[0]!.id,
        disposition: "accept_risk",
        reason: "Accepted by operator",
      })) as { readonly revision: number };
      assert.isTrue(resolved.revision > detail.revision);

      const after = await getCard(harness.rpc, card.cardId);
      const afterFinding = after.reviewReport?.findings.find((f) => f.id === findings[0]!.id);
      assert.strictEqual(afterFinding?.status, "resolved");
      assert.strictEqual(afterFinding?.disposition, "accept_risk");
    }),
);

it.live(
  "[F17-T2-1] Tier 2: agent/MCP token is rejected on workflowBoard.resolveFinding with client:interactive error",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const agentClient = await mintAgentWsClient(harness, "thr-agent-17");
      try {
        const msg = await expectUnaryFailureAny(
          agentClient,
          "workflowBoard.resolveFinding",
          {
            cardId: "card-f17-auth",
            expectedRevision: 1,
            findingId: "FND-1",
            disposition: "accept_risk",
            note: "Agent cannot self-resolve",
          },
          ["EnvironmentAuthorizationError"],
        );
        assert.include(msg, "client:interactive");
      } finally {
        agentClient.close();
      }
    }),
);

it.live(
  "[F17-T2-2] Tier 2 (Gated): snapshot mutation after PASS review marks review receipt stale and blocks Ship",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f17-stale-rev");
      await createProject(
        harness.rpc,
        counter,
        "proj-f17-stale",
        "F17 Stale",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f17-stale", wtPath);
      const revReady = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);
      await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: revReady,
        criterionId: "AC-2",
        verdict: "pass",
        note: "Verified before snapshot edit",
      });

      NodeFS.writeFileSync(NodePath.join(wtPath, "README.md"), "# post-review change\n", "utf8");
      const staleDetail = await getCard(harness.rpc, card.cardId);
      assert.strictEqual(staleDetail.reviewReport?.status, "stale");

      const staleMsg = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.approveDelivery",
        { cardId: card.cardId, expectedRevision: staleDetail.revision },
        "WorkflowBoardRequestError",
      );
      assert.include(staleMsg.toLowerCase(), "stale");
    }),
);

it.live(
  "[F17-T2-3] Tier 2 (Gated): re-review after finding fix preserves prior finding and disposition in card history",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f17-history");
      await createProject(harness.rpc, counter, "proj-f17-hist", "F17 Hist", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f17-hist", wtPath, {
        cardTitle: "Re-review Finding History [REVIEW_FINDING]",
      });

      await installMockProviders(harness);
      const { started: firstRun, card: afterFirstReview } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "review",
        modelSelection: { instanceId: "mock-review-findings", model: MOCK_CLAUDE_MODEL },
      });
      const firstFindingId = afterFirstReview.reviewReport?.findings[0]?.id;
      assert(firstFindingId !== undefined, "Expected independent review to persist a finding");

      const resolved = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.resolveFinding",
        {
          cardId: card.cardId,
          expectedRevision: afterFirstReview.revision,
          findingId: firstFindingId,
          disposition: "fix",
          reason: "Fixed in follow-up commit",
        },
      );
      const { started: secondRun, card: afterSecondReview } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: resolved.revision,
        stage: "review",
        modelSelection: { instanceId: "mock-review-pass", model: MOCK_CLAUDE_MODEL },
      });
      assert.notStrictEqual(secondRun.threadId, firstRun.threadId);
      assert.strictEqual(afterSecondReview.reviewReport?.verdict, "pass");

      const db = harness.openReadOnlyDb();
      try {
        const preserved = db
          .prepare(
            "SELECT status, disposition FROM workflow_board_review_findings WHERE id = ? AND card_id = ?",
          )
          .get(firstFindingId, card.cardId);
        assert.strictEqual(preserved?.status, "historical");
        assert.strictEqual(preserved?.disposition, "fix");
      } finally {
        db.close();
      }
    }),
);

it.live(
  "[F18-T1-1] Tier 1 (Gated): approveDelivery succeeds once checks, fresh review, and AC observations are satisfied",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f18-ship");
      await createProject(harness.rpc, counter, "proj-f18", "F18 Project", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f18", wtPath);
      const revAfterReview = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);

      const obs = (await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: revAfterReview,
        criterionId: "AC-2",
        verdict: "pass",
        note: "Verified manually",
      })) as { readonly revision: number };

      const shipped = (await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: card.cardId,
        expectedRevision: obs.revision,
      })) as {
        readonly deliveryStatus: string;
        readonly card: { readonly stage: string };
        readonly deliveryReceipt: { readonly productSnapshotDigest: string };
      };
      assert.strictEqual(shipped.card.stage, "ship");
      assert.strictEqual(shipped.deliveryStatus, "shipped");
      assert.isString(shipped.deliveryReceipt.productSnapshotDigest);
    }),
);

it.live(
  "[F18-T1-2] Tier 1 (Gated): Ship gate blocks failed/stale checks or unresolved findings with actionable fix target",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f18-block");
      await createProject(harness.rpc, counter, "proj-f18-blk", "F18 Block", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f18-blk", wtPath, {
        checkCommand: ["node", "-e", "process.exit(1)"],
      });

      // Complete every task first so the failed check is the gate that reports.
      const { card: afterTsk1 } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
      });
      const { card: afterTsk2 } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: afterTsk1.revision,
        stage: "execute",
        taskId: "TSK-2",
      });
      await retryCheck(harness.rpc, card.cardId, afterTsk2.revision, "CHK-1");

      const detail = await getCard(harness.rpc, card.cardId);
      assert.strictEqual(
        detail.checkReceipts?.find((r) => r.checkId === "CHK-1")?.status,
        "failed",
      );
      const blocker = detail.shipReadiness?.blockers.find((b) => b.kind === "check_failed");
      assert.isDefined(blocker);
      assert.strictEqual(blocker?.pointer.stage, "execute");
      assert.strictEqual(blocker?.pointer.targetId, "CHK-1");

      const blockMsg = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.approveDelivery",
        { cardId: card.cardId, expectedRevision: detail.revision },
        "WorkflowBoardRequestError",
      );
      const blockLower = blockMsg.toLowerCase();
      assert.isTrue(blockLower.includes("check") || blockLower.includes("chk-1"));
    }),
);

it.live(
  "[F18-T1-3] Tier 1 (Gated): per-AC pass, fail, and not-applicable observations enforce Ship eligibility",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f18-ac");
      await createProject(harness.rpc, counter, "proj-f18-ac", "F18 AC", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f18-ac", wtPath);
      // Satisfy tasks, checks, and review so the AC-2 observation is the deciding gate.
      const revAfterReview = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);

      const failedObs = (await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: revAfterReview,
        criterionId: "AC-2",
        verdict: "fail",
        note: "UI alignment failed",
      })) as { readonly revision: number };

      const acMsg = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.approveDelivery",
        { cardId: card.cardId, expectedRevision: failedObs.revision },
        "WorkflowBoardRequestError",
      );
      assert.include(acMsg, "AC-2");

      const naObs = (await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: failedObs.revision,
        criterionId: "AC-2",
        verdict: "not-applicable",
        note: "Backend-only change",
      })) as { readonly revision: number };
      assert.isTrue(naObs.revision > failedObs.revision);

      const shipped = (await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: card.cardId,
        expectedRevision: naObs.revision,
      })) as { readonly deliveryStatus: string };
      assert.strictEqual(shipped.deliveryStatus, "shipped");
    }),
);

it.live(
  "[F18-T2-1] Tier 2 (Gated): missing manual criterion observation blocks approveDelivery until recorded",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f18-missing-ac");
      await createProject(harness.rpc, counter, "proj-f18-miss", "F18 Miss", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f18-miss", wtPath);

      const readyRevision = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);
      const missMsg = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.approveDelivery",
        { cardId: card.cardId, expectedRevision: readyRevision },
        "invalid_stage_transition",
      );
      assert.include(missMsg, "AC-2");
    }),
);

it.live(
  "[F18-T2-2] Tier 2 (Gated): snapshot drift right before approveDelivery rejects due to stale evidence",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f18-drift");
      await createProject(
        harness.rpc,
        counter,
        "proj-f18-drift",
        "F18 Drift",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f18-drift", wtPath);
      const revReady = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);

      await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: revReady,
        criterionId: "AC-2",
        verdict: "pass",
        note: "Verified before drift",
      });

      NodeFS.writeFileSync(NodePath.join(wtPath, "README.md"), "# late drift\n", "utf8");
      const staleDetail = await getCard(harness.rpc, card.cardId);
      assert.strictEqual(
        staleDetail.checkReceipts?.find((r) => r.checkId === "CHK-1")?.status,
        "stale",
      );
      assert.strictEqual(staleDetail.reviewReport?.status, "stale");
      const driftMsg = await expectUnaryFailure(
        harness.rpc,
        "workflowBoard.approveDelivery",
        { cardId: card.cardId, expectedRevision: staleDetail.revision },
        "WorkflowBoardRequestError",
      );
      assert.include(driftMsg.toLowerCase(), "stale");
    }),
);

it.live(
  "[F18-T2-3] Tier 2 (Gated): shipped card and delivery receipt remain queryable in completed view after restart()",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f18-persist");
      await createProject(
        harness.rpc,
        counter,
        "proj-f18-pers",
        "F18 Persist",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f18-pers", wtPath);
      const revAfterReview = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);

      const obs = (await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: revAfterReview,
        criterionId: "AC-2",
        verdict: "pass",
        note: "Verified",
      })) as { readonly revision: number };

      await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: card.cardId,
        expectedRevision: obs.revision,
      });

      await harness.restart();
      const reloaded = (await harness.rpc.callUnary("workflowBoard.getCard", {
        cardId: card.cardId,
      })) as {
        readonly stage: string;
        readonly status: string;
        readonly deliveryReceipt: unknown;
      };
      assert.strictEqual(reloaded.stage, "ship");
      assert.strictEqual(reloaded.status, "completed");
      const decodeReceipt = Schema.decodeUnknownSync(
        Schema.toCodecJson(WorkflowBoardDeliveryReceipt),
      );
      const receipt = decodeReceipt(reloaded.deliveryReceipt);
      assert.strictEqual(receipt.status, "shipped");

      NodeFS.writeFileSync(NodePath.join(wtPath, "README.md"), "# Changed after approval\n");
      const afterDrift = await harness.rpc.callUnary<{ readonly deliveryReceipt: unknown }>(
        "workflowBoard.getCard",
        { cardId: card.cardId },
      );
      assert.deepStrictEqual(decodeReceipt(afterDrift.deliveryReceipt), {
        ...receipt,
        status: "stale",
      });
    }),
);

it.live(
  "[F19-T1-1] Tier 1: git.runStackedAction commit commits worktree changes without triggering push or PR",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const wtPath = harness.git!.createWorktree("feat/f19-commit");
      NodeFS.writeFileSync(NodePath.join(wtPath, "shipped.txt"), "feature change\n", "utf8");

      const chunks = await runStackedGitAction(harness.rpc, {
        actionId: "act-f19-commit-1",
        cwd: wtPath,
        action: "commit",
        commitMessage: "feat: ship card change",
      });

      const finished = chunks.find((c) => c.kind === "action_finished");
      assert.isDefined(finished?.result);
      assert.strictEqual(finished?.result?.commit.status, "created");
      assert.strictEqual(finished?.result?.push.status, "skipped_not_requested");
      assert.strictEqual(finished?.result?.pr.status, "skipped_not_requested");
      const headSha = runGit(wtPath, ["rev-parse", "HEAD"]);
      assert.strictEqual(finished?.result?.commit.commitSha, headSha);
    }),
);

it.live(
  "[F19-T1-2] Tier 1 (Gated): git.runStackedAction push updates bare remote without implicit commit or PR",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3GitDeliveryCapability(harness);
      const bareRemote = initBareRemote(NodePath.join(harness.tempRoot, "origin.git"));
      const wtPath = harness.git!.createWorktree("feat/f19-push");
      runGit(wtPath, ["remote", "add", "origin", bareRemote]);
      const sha = harness.git!.commitFile(wtPath, "push.txt", "push me\n", "commit before push");

      const chunks = await runStackedGitAction(harness.rpc, {
        actionId: "act-f19-push-1",
        cwd: wtPath,
        action: "push",
      });
      const finished = chunks.find((c) => c.kind === "action_finished");
      assert.strictEqual(finished?.result?.commit.status, "skipped_not_requested");
      assert.strictEqual(finished?.result?.push.status, "pushed");
      assert.strictEqual(finished?.result?.pr.status, "skipped_not_requested");
      const remoteSha = runGit(bareRemote, ["rev-parse", "refs/heads/feat/f19-push"]);
      assert.strictEqual(remoteSha, sha);
    }),
);

it.live(
  "[F19-T1-3] Tier 1 (Gated): git.runStackedAction create_pr stays isolated from commit and push",
  () =>
    Effect.gen(function* () {
      const repoRoot = resolveWorkflowBoardRepoRoot();
      const platform = yield* HostProcess.Platform;
      const env = yield* HostProcess.Environment;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env });
      assert.isDefined(binaryPath, "Expected built legioncode binary");
      yield* Effect.promise(async () => {
        using ghShim = createFakeGhBinDir();
        await using harness = await startWorkflowBoardE2eHarness({
          repoRoot,
          platform,
          binaryPath,
          seedGitWorkspace: true,
          env: { PATH: `${ghShim.binDir}${NodePath.delimiter}${HOST_PATH}` },
        });
        await assertM3GitDeliveryCapability(harness);
        const bareRemote = initBareRemote(NodePath.join(harness.tempRoot, "origin-pr.git"));
        const wtPath = harness.git!.createWorktree("feat/f19-pr");
        runGit(wtPath, ["remote", "add", "origin", bareRemote]);
        harness.git!.commitFile(wtPath, "pr.txt", "pr content\n", "commit before create_pr");
        runGit(wtPath, ["push", "-u", "origin", "feat/f19-pr"]);
        const beforeSha = runGit(wtPath, ["rev-parse", "HEAD"]);

        const chunks = await runStackedGitAction(harness.rpc, {
          actionId: "act-f19-pr-1",
          cwd: wtPath,
          action: "create_pr",
        });
        const finished = chunks.find((c) => c.kind === "action_finished");
        assert.isDefined(finished?.result);
        assert.strictEqual(finished?.result?.commit.status, "skipped_not_requested");
        assert.strictEqual(finished?.result?.push.status, "skipped_not_requested");
        assert.strictEqual(finished?.result?.pr.status, "created");
        assert.strictEqual(runGit(wtPath, ["rev-parse", "HEAD"]), beforeSha);
      });
    }),
);

it.live(
  "[F19-T2-1] Tier 2: launching a thread and writing workspace artifacts never auto-commits changes",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f19-no-autocommit");
      const headBefore = runGit(wtPath, ["rev-parse", "HEAD"]);

      await createProject(harness.rpc, counter, "proj-f19-noauto", "No Auto", harness.workspaceDir);
      const artifactDir = NodePath.join(wtPath, ".legion", "workflows", "card-19");
      NodeFS.mkdirSync(artifactDir, { recursive: true });
      NodeFS.writeFileSync(NodePath.join(artifactDir, "spec.md"), "# Spec\n", "utf8");

      const launched = (await harness.rpc.callUnary("orchestration.launchThread", {
        commandId: nextId(counter, "cmd-noauto"),
        projectId: "proj-f19-noauto",
        title: "Verify No Auto-Commit",
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        runtimeMode: "auto-accept-edits",
        interactionMode: "default",
        workspaceStrategy: { type: "existing_worktree", worktreePath: wtPath },
        initialMessage: {
          messageId: nextId(counter, "msg"),
          text: "Inspect spec",
          attachments: [],
        },
      })) as { readonly threadId: string };
      await awaitThreadRunSettled(harness.rpc, launched.threadId);

      assert.strictEqual(runGit(wtPath, ["rev-parse", "HEAD"]), headBefore);
      const status = runGit(wtPath, ["status", "--porcelain"]);
      assert.include(status, ".legion/");
    }),
);

it.live(
  "[F19-T2-2] Tier 2: clean commit reports no changes, and nonexistent cwd fails without a commit",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const wtPath = harness.git!.createWorktree("feat/f19-clean");
      const headBefore = runGit(wtPath, ["rev-parse", "HEAD"]);

      const cleanChunks = await runStackedGitAction(harness.rpc, {
        actionId: "act-f19-clean",
        cwd: wtPath,
        action: "commit",
        commitMessage: "should fail cleanly",
      });
      assert.strictEqual(
        cleanChunks.find((c) => c.kind === "action_finished")?.result?.commit.status,
        "skipped_no_changes",
      );
      assert.strictEqual(runGit(wtPath, ["rev-parse", "HEAD"]), headBefore);

      const missingChunks = await runStackedGitAction(harness.rpc, {
        actionId: "act-f19-missing",
        cwd: NodePath.join(harness.tempRoot, "does-not-exist"),
        action: "commit",
      });
      assert.isTrue(missingChunks.some((c) => c.kind === "action_failed"));
    }),
);

it.live(
  "[F19-T2-3] Tier 2 (Gated): approveDelivery leaves worktree uncommitted until explicit git.runStackedAction",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f19-manual-git");
      await createProject(harness.rpc, counter, "proj-f19-man", "F19 Manual", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f19-man", wtPath);
      NodeFS.writeFileSync(NodePath.join(wtPath, "feature.txt"), "uncommitted\n", "utf8");
      const revAfterReview = await runExecuteAndReviewForShip(harness, card.cardId, card.revision);
      const headBefore = runGit(wtPath, ["rev-parse", "HEAD"]);

      const obs = (await harness.rpc.callUnary("workflowBoard.recordManualAcceptance", {
        cardId: card.cardId,
        expectedRevision: revAfterReview,
        criterionId: "AC-2",
        verdict: "pass",
        note: "OK",
      })) as { readonly revision: number };
      await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: card.cardId,
        expectedRevision: obs.revision,
      });

      assert.strictEqual(runGit(wtPath, ["rev-parse", "HEAD"]), headBefore);
      assert.include(runGit(wtPath, ["status", "--porcelain"]), "feature.txt");
    }),
);

it.live(
  "[F20-T1-1] Tier 1: SQLite migration head >= 110, core tables persist across restart(), and board capability probes cleanly",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      const head = harness.readSqlMigrationHead();
      assert.isDefined(head);
      assert.isTrue((head ?? 0) >= 110);
      assert.strictEqual(typeof harness.environment.projectWorkflowBoard, "boolean");

      await harness.restart();
      const db = harness.openReadOnlyDb();
      try {
        const rows = db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all();
        const tables = new Set(rows.map((r) => String(r.name)));
        assert.isTrue(tables.has("orchestration_v2_projection_threads"));
        assert.isTrue(tables.has("workflow_runs"));
        assert.isTrue(tables.has("workflow_engine_journal"));
      } finally {
        db.close();
      }
    }),
);

it.live(
  "[F20-T1-2] Tier 1: WS rejects non-loopback/untrusted-Origin upgrades and legacy server rejects unbuilt board RPCs cleanly",
  () =>
    runHarnessEffect({ seedGitWorkspace: false }, async (harness) => {
      const wsOrigin = harness.origin.replace(/^http/, "ws");
      const evilOriginStatus = await connectRawWsStatus(`${wsOrigin}/ws?orchestrationProtocol=2`, {
        Origin: "https://evil.example",
      });
      assert.strictEqual(evilOriginStatus, 403);

      const proxyStatus = await connectRawWsStatus(`${wsOrigin}/ws?orchestrationProtocol=2`, {
        "x-forwarded-for": "203.0.113.9",
      });
      assert.strictEqual(proxyStatus, 401);

      if (!harness.environment.projectWorkflowBoard) {
        await expectUnaryFailure(
          harness.rpc,
          "workflowBoard.getCard",
          { cardId: "card-nonexistent" },
          "UnsupportedRpcMethod",
        );
      } else {
        await expectUnaryFailureAny(
          harness.rpc,
          "workflowBoard.getCard",
          { cardId: "card-nonexistent" },
          ["WorkflowBoardRequestError", "card_not_found"],
        );
      }
    }),
);

it.live(
  "[F20-T1-3] Tier 1 (Gated): .legion/workflows/<card-id>/ stores Git-readable deliverables while receipts remain host-only in SQLite",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f20-files");
      await createProject(harness.rpc, counter, "proj-f20", "F20 Project", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f20", wtPath);

      const cardDir = NodePath.join(wtPath, ".legion", "workflows", card.cardId);
      assert.isTrue(NodeFS.existsSync(NodePath.join(cardDir, "spec.md")));
      assert.isTrue(NodeFS.existsSync(NodePath.join(cardDir, "plan.md")));
      assert.isFalse(NodeFS.existsSync(NodePath.join(cardDir, "receipts.sqlite")));
      assert.isTrue((harness.readSqlMigrationHead() ?? 0) > 110);
    }),
);

it.live(
  "[F20-T2-1] Tier 2 (Gated): external disk edit to .legion/workflows/<card-id>/spec.md triggers reconciliation and stales approvals",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f20-ext-edit");
      await createProject(harness.rpc, counter, "proj-f20-ext", "F20 Ext", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f20-ext", wtPath);

      const specPath = NodePath.join(wtPath, ".legion", "workflows", card.cardId, "spec.md");
      NodeFS.appendFileSync(specPath, "\n- External manual edit\n", "utf8");

      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.startStage",
        { cardId: card.cardId, expectedRevision: card.revision, stage: "execute" },
        ["WorkflowBoardRequestError", "reconcile"],
      );
    }),
);

it.live(
  "[F20-T2-2] Tier 2 (Gated): Spec edit invalidates Spec+Plan approvals whereas Plan-only edit preserves Spec approval",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f20-digest");
      await createProject(harness.rpc, counter, "proj-f20-dig", "F20 Digest", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f20-dig", wtPath);

      const planEdited = await harness.rpc.callUnary<{
        readonly revision: number;
        readonly card: {
          readonly specApproval?: { readonly status: string };
          readonly planApproval?: { readonly status: string };
        };
      }>("workflowBoard.updatePlan", {
        cardId: card.cardId,
        expectedRevision: card.revision,
        tasks: [
          {
            taskId: "TSK-1",
            title: "Plan-only edit task",
            dependsOn: [],
            filesAllowed: ["src/ok.ts"],
            testingMethod: "regression-first",
          },
        ],
      });
      assert.strictEqual(planEdited.card.specApproval?.status, "valid");
      assert.strictEqual(planEdited.card.planApproval?.status, "stale");
      const reapproved = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.approvePlan",
        {
          cardId: card.cardId,
          expectedRevision: planEdited.revision,
        },
      );
      const specEdited = await harness.rpc.callUnary<{
        readonly card: {
          readonly specApproval?: { readonly status: string };
          readonly planApproval?: { readonly status: string };
        };
      }>("workflowBoard.updateSpec", {
        cardId: card.cardId,
        expectedRevision: reapproved.revision,
        brief: "Updated spec",
      });
      assert.strictEqual(specEdited.card.specApproval?.status, "stale");
      assert.strictEqual(specEdited.card.planApproval?.status, "stale");
    }),
);

it.live(
  "[F20-T2-3] Tier 2 (Gated): SQLite card identity, revision ledger, and prior run links survive multiple host restarts",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f20-restart");
      await createProject(
        harness.rpc,
        counter,
        "proj-f20-rst",
        "F20 Restart",
        harness.workspaceDir,
      );
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f20-rst", wtPath);

      await harness.restart();
      await harness.restart();
      const loaded = (await harness.rpc.callUnary("workflowBoard.getCard", {
        cardId: card.cardId,
      })) as { readonly cardId: string; readonly revision: number };
      assert.strictEqual(loaded.cardId, card.cardId);
      assert.strictEqual(loaded.revision, card.revision);
    }),
);

it.live(
  "[F20-T2-4] Tier 2 (Gated): write-journal crash recovery reconciles .legion/workflows/<card-id>/ and SQLite receipts across restart()",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f20-journal");
      await createProject(harness.rpc, counter, "proj-f20-jrn", "F20 Jrn", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f20-jrn", wtPath);

      const specPath = NodePath.join(wtPath, ".legion", "workflows", card.cardId, "spec.md");
      assert.isTrue(NodeFS.existsSync(specPath));
      const originalSpecContent = NodeFS.readFileSync(specPath, "utf8");
      const specSha = NodeCrypto.createHash("sha256")
        .update(originalSpecContent, "utf8")
        .digest("hex");

      const tornTmpPath = NodePath.join(wtPath, ".legion", "workflows", card.cardId, "spec.md.tmp");
      NodeFS.writeFileSync(tornTmpPath, "uncommitted-torn-write\n", "utf8");

      await harness.restart();
      const afterRestart = (await harness.rpc.callUnary("workflowBoard.getCard", {
        cardId: card.cardId,
      })) as {
        readonly cardId: string;
        readonly specApproval?: { readonly specDigest?: string; readonly status?: string };
      };
      assert.strictEqual(afterRestart.specApproval?.status, "valid");
      assert.strictEqual(afterRestart.specApproval?.specDigest?.replace(/^sha256:/, ""), specSha);
      assert.strictEqual(NodeFS.readFileSync(specPath, "utf8"), originalSpecContent);

      const art = (await harness.rpc.callUnary("workflowBoard.getArtifact", {
        cardId: card.cardId,
        key: "spec.md",
      })) as { readonly content: string };
      assert.strictEqual(art.content, originalSpecContent);
      assert.notInclude(art.content, "uncommitted-torn-write");

      const db = harness.openReadOnlyDb();
      try {
        const pending = db
          .prepare(
            "SELECT COUNT(*) AS cnt FROM workflow_board_write_journal WHERE status NOT IN ('committed', 'rolled_back')",
          )
          .get() as { readonly cnt: number };
        assert.strictEqual(pending.cnt, 0);
      } finally {
        db.close();
      }

      NodeFS.unlinkSync(specPath);
      await harness.restart();
      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.startStage",
        { cardId: card.cardId, expectedRevision: card.revision, stage: "execute" },
        ["WorkflowBoardRequestError", "missing", "reconcile"],
      );
    }),
);

it.live(
  "[F20-T2-5] Tier 2 (Gated): concurrent Promise.all expectedRevision updates serialize with single winner and RevisionConflict",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f20-race");
      await createProject(harness.rpc, counter, "proj-f20-race", "F20 Race", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f20-race", wtPath);

      const results = await Promise.allSettled([
        harness.rpc.callUnary("workflowBoard.updatePlan", {
          cardId: card.cardId,
          expectedRevision: card.revision,
          plan: {
            tasks: [
              {
                id: "TSK-1",
                title: "Updated task",
                dependsOn: [],
                filesAllowed: ["src/ok.ts"],
                filesForbidden: [],
              },
            ],
            checks: [{ id: "CHK-1", criterionId: "AC-1", argv: ["node", "-e", "process.exit(0)"] }],
            criterionMappings: [
              { criterionId: "AC-1", method: "automated", checkIds: ["CHK-1"] },
              { criterionId: "AC-2", method: "manual" },
            ],
          },
        }),
        harness.rpc.callUnary("workflowBoard.updateSpec", {
          cardId: card.cardId,
          expectedRevision: card.revision,
          brief: "Concurrent spec race",
          acceptanceCriteria: [],
        }),
      ]);

      const fulfilled = results.filter((r) => r.status === "fulfilled");
      const rejected = results.filter((r) => r.status === "rejected");
      assert.strictEqual(fulfilled.length, 1);
      assert.strictEqual(rejected.length, 1);
      const errText = String(rejected[0]?.reason).toLowerCase();
      assert.isTrue(errText.includes("revision"));
    }),
);

it.live(
  "[F21-T1-1] Tier 1: workflows.subscribe and workflows.counts stream run-inspector snapshots cleanly",
  () =>
    runHarnessEffect({ seedGitWorkspace: false }, async (harness) => {
      const sub = harness.rpc.subscribe("workflows.subscribe", {});
      const snapshot = (await sub.nextChunk()) as {
        readonly workflows: ReadonlyArray<unknown>;
        readonly runs: ReadonlyArray<unknown>;
        readonly questions: ReadonlyArray<unknown>;
      };
      assert.isArray(snapshot.workflows);
      assert.isArray(snapshot.runs);
      assert.isArray(snapshot.questions);
      await sub.interrupt();

      const countsSub = harness.rpc.subscribe("workflows.counts", {});
      const counts = (await countsSub.nextChunk()) as {
        readonly needsYou: number;
        readonly active: number;
        readonly total: number;
      };
      assert.strictEqual(typeof counts.needsYou, "number");
      assert.strictEqual(typeof counts.active, "number");
      assert.strictEqual(typeof counts.total, "number");
      await countsSub.interrupt();
    }),
);

it.live(
  "[F21-T1-2] Tier 1 (Gated): stage job assembles brief with approved digests, acceptance criteria, constraints, source refs, and expected output",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f21-brief");
      await createProject(harness.rpc, counter, "proj-f21-br", "F21 Brief", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f21-br", wtPath, {
        constraints: ["Must stay under 50ms"],
        sourceRefs: ["README.md"],
      });

      const started = (await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
      })) as { readonly threadId: string };
      const proj = decodeThreadProjection(
        await harness.rpc.callUnary("orchestration.getThreadProjection", {
          threadId: started.threadId,
        }),
      );
      const briefText = proj.messages.map((m) => m.text).join("\n");
      assert.include(briefText, "AC-1");
      assert.include(briefText, "Must stay under 50ms");
      assert.include(briefText, "README.md");
      assert.match(briefText, /Approved Spec Digest: `sha256:[0-9a-f]{64}`/);
      assert.match(briefText, /Approved Plan Digest: `sha256:[0-9a-f]{64}`/);
      assert.include(briefText, "Expected Execute Output Contract");
    }),
);

it.live(
  "[F21-T1-3] Tier 1 (Gated): card survives provider/model switch across stage jobs and links runs in workflows.get",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f21-switch");
      await createProject(harness.rpc, counter, "proj-f21-sw", "F21 Switch", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f21-sw", wtPath);

      await installMockProviders(harness);
      const { started: s1, card: after1 } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
        taskId: "TSK-1",
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      });
      assert.strictEqual(
        after1.taskExecutions?.find((task) => task.taskId === "TSK-1")?.status,
        "completed",
      );

      const { started: s2 } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: after1.revision,
        stage: "review",
        modelSelection: { instanceId: "mock-review-pass", model: MOCK_CLAUDE_MODEL },
      });
      assert.isString(s1.runId);
      assert.isString(s2.runId);
      assert.notStrictEqual(s1.runId, s2.runId);

      const sub = harness.rpc.subscribe("workflows.subscribe", {});
      const snap = (await sub.nextChunk()) as {
        readonly runs: ReadonlyArray<{ readonly runId: string }>;
      };
      await sub.interrupt();
      assert.isTrue(snap.runs.some((r) => r.runId === s1.runId));
      assert.isTrue(snap.runs.some((r) => r.runId === s2.runId));

      const runDetail1 = (await harness.rpc.callUnary("workflows.get", {
        runId: s1.runId,
      })) as { readonly run: { readonly runId: string; readonly version: number } };
      const runDetail2 = (await harness.rpc.callUnary("workflows.get", {
        runId: s2.runId,
      })) as { readonly run: { readonly runId: string; readonly version: number } };
      assert.strictEqual(runDetail1.run.runId, s1.runId);
      assert.strictEqual(runDetail2.run.runId, s2.runId);
      assert.isTrue(runDetail1.run.version >= 1);
      assert.isTrue(runDetail2.run.version >= 1);
    }),
);

it.live(
  "[F21-T2-1] Tier 2: workflows.get and workflows.cancel reject nonexistent runId cleanly with WorkflowRequestError",
  () =>
    runHarnessEffect({ seedGitWorkspace: false }, async (harness) => {
      await expectUnaryFailure(
        harness.rpc,
        "workflows.get",
        { runId: "missing-run-e2e-f21" },
        "WorkflowRequestError",
      );
      await expectUnaryFailure(
        harness.rpc,
        "workflows.cancel",
        { runId: "missing-run-e2e-f21", reason: "cleanup" },
        "WorkflowRequestError",
      );
    }),
);

it.live(
  "[F21-T2-2] Tier 2 (Gated): returnForRevision re-assembles stage brief with updated revision and preserves run history",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f21-return");
      await createProject(harness.rpc, counter, "proj-f21-ret", "F21 Return", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f21-ret", wtPath);
      const { started: firstExec, card: preRet } = await startStageSettled(harness.rpc, {
        cardId: card.cardId,
        expectedRevision: card.revision,
        stage: "execute",
      });

      const returned = (await harness.rpc.callUnary("workflowBoard.returnForRevision", {
        cardId: card.cardId,
        expectedRevision: preRet.revision,
        targetStage: "spec",
        reason: "Clarify AC-1 boundary",
      })) as { readonly stage: string; readonly revision: number };
      assert.strictEqual(returned.stage, "spec");
      assert.isTrue(returned.revision > preRet.revision);

      const specUp = (await harness.rpc.callUnary("workflowBoard.updateSpec", {
        cardId: card.cardId,
        expectedRevision: returned.revision,
        brief: "Updated AC-1 boundary brief",
        acceptanceCriteria: [{ id: "AC-1", text: "Updated AC-1 boundary" }],
      })) as { readonly revision: number };
      const specOk = (await harness.rpc.callUnary("workflowBoard.approveSpec", {
        cardId: card.cardId,
        expectedRevision: specUp.revision,
      })) as { readonly revision: number };
      const planOk = (await harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: card.cardId,
        expectedRevision: specOk.revision,
      })) as { readonly revision: number };
      const secondExec = (await harness.rpc.callUnary("workflowBoard.startStage", {
        cardId: card.cardId,
        expectedRevision: planOk.revision,
        stage: "execute",
      })) as { readonly threadId: string; readonly runId: string };
      assert.notStrictEqual(secondExec.runId, firstExec.runId);
      const proj = decodeThreadProjection(
        await harness.rpc.callUnary("orchestration.getThreadProjection", {
          threadId: secondExec.threadId,
        }),
      );
      const briefText = proj.messages.map((m) => m.text).join("\n");
      assert.include(briefText, "Updated AC-1 boundary");
      const detail = (await harness.rpc.callUnary("workflowBoard.getCard", {
        cardId: card.cardId,
      })) as {
        readonly revisions: ReadonlyArray<unknown>;
        readonly stageRuns: ReadonlyArray<unknown>;
      };
      assert.isTrue(detail.revisions.length > 1);
      assert.isTrue(detail.stageRuns.length >= 2);
    }),
);

it.live(
  "[F21-T2-3] Tier 2 (Gated): invalid stage target on workflowBoard.startStage rejects cleanly without corrupting card",
  () =>
    runHarnessEffect({ seedGitWorkspace: true }, async (harness) => {
      await assertM3ExecuteShipCapability(harness);
      const counter: Counter = { value: 0 };
      const wtPath = harness.git!.createWorktree("feat/f21-bad-stage");
      await createProject(harness.rpc, counter, "proj-f21-bad", "F21 Bad", harness.workspaceDir);
      const card = await setupApprovedPlanCard(harness.rpc, counter, "proj-f21-bad", wtPath);

      await expectUnaryFailureAny(
        harness.rpc,
        "workflowBoard.startStage",
        { cardId: card.cardId, expectedRevision: card.revision, stage: "nonexistent-stage" },
        ["ParseError", "WorkflowBoardRequestError", "stage"],
      );
    }),
);

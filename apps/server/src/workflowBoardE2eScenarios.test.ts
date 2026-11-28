// @effect-diagnostics nodeBuiltinImport:off -- E2E scenarios spawn isolated Rust servers, inspect git worktrees, and verify disk artifacts.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, describe, it } from "@effect/vitest";
import {
  OrchestrationV2RpcSchemas,
  type ProjectWorkflowBoardSnapshot,
  type WorkflowBoardApproveSpecResult,
  type WorkflowBoardArtifact,
  type WorkflowBoardChallengeStepResult,
  type WorkflowBoardExploreStepResult,
  type WorkflowBoardResolveFindingResult,
  type WorkflowBoardResumeCardResult,
  type WorkflowBoardRetryStepResult,
  type WorkflowBoardStartStageResult,
  type WorkflowBoardUpdateSpecResult,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";
import {
  awaitWorkflowBoardStageSettlement,
  installWorkflowBoardTestProvider,
  resolveWorkflowBoardRepoRoot,
  startWorkflowBoardE2eHarness,
  WorkflowBoardRpcClient,
  type WorkflowBoardE2eHarness,
} from "./workflowBoardE2eHarness.ts";

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

function runScenarioTest(
  run: (harness: WorkflowBoardE2eHarness, binaryPath: string) => Promise<void>,
) {
  return () =>
    Effect.gen(function* () {
      const repoRoot = resolveWorkflowBoardRepoRoot();
      const platform = yield* HostProcess.Platform;
      const env = yield* HostProcess.Environment;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env });
      if (binaryPath === undefined) {
        throw new Error("Build the Rust CLI before running workflow board scenarios.");
      }

      yield* Effect.promise(async () => {
        await using harness = await startWorkflowBoardE2eHarness({
          repoRoot,
          platform,
          binaryPath,
          seedGitWorkspace: true,
        });
        assert.isTrue(harness.environment.projectWorkflowBoard);
        await harness.rpc.callUnary("server.updateSettings", { patch: { workflowsEnabled: true } });
        await run(harness, binaryPath);
      });
    });
}

const decodeThreadProjection = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.getThreadProjection.output),
);

function settleStage(rpc: WorkflowBoardRpcClient, stage: WorkflowBoardStartStageResult) {
  return awaitWorkflowBoardStageSettlement(rpc, stage.cardId, stage.stageRunId);
}

async function awaitPlanTurn(rpc: WorkflowBoardRpcClient, threadId: string) {
  const sub = rpc.subscribe("orchestration.subscribeThread", {
    threadId,
    afterSequence: 0,
    requestCompletionMarker: true,
    acceptBoundedSnapshot: true,
  });
  try {
    const readAfterUpdate = async (): Promise<void> => {
      const projection = decodeThreadProjection(
        await rpc.callUnary("orchestration.getThreadProjection", { threadId }),
      );
      const terminal = projection.runs.find(
        (run) =>
          run.status === "completed" || run.status === "failed" || run.status === "interrupted",
      );
      if (terminal) {
        assert.strictEqual(terminal.status, "completed");
        return;
      }
      await sub.nextChunk();
      await readAfterUpdate();
    };
    await readAfterUpdate();
  } finally {
    await sub.interrupt();
  }
}

async function runCheck(rpc: WorkflowBoardRpcClient, card: WorkflowCardDetail) {
  const result = await rpc.callUnary<WorkflowBoardRetryStepResult>("workflowBoard.retryStep", {
    cardId: card.cardId,
    expectedRevision: card.revision,
    stepKind: "check",
    targetId: "CHK-1",
  });
  return result.card;
}

function checkScript(harness: WorkflowBoardE2eHarness, marker: string) {
  const script = NodePath.join(harness.tempRoot, "verify-marker.cjs");
  NodeFS.writeFileSync(
    script,
    "process.exit(require('node:fs').existsSync(process.argv[2]) ? 0 : 1);",
    "utf8",
  );
  return [process.execPath, script, marker];
}

function installReviewer(harness: WorkflowBoardE2eHarness, withFinding = false) {
  return installWorkflowBoardTestProvider(
    harness,
    "scenario-reviewer",
    JSON.stringify({
      verdict: withFinding ? "findings" : "pass",
      summary: "Independent scenario review.",
      findings: withFinding
        ? [
            {
              id: "FIND-1",
              severity: "minor",
              criterionId: "AC-02",
              message: "Document the operator action.",
            },
          ]
        : [],
    }),
  );
}

async function createProject(
  harness: WorkflowBoardE2eHarness,
  counter: Counter,
  projectId: string,
  title: string,
): Promise<string> {
  await harness.rpc.callUnary("orchestration.v2.dispatchCommand", {
    type: "project.create",
    commandId: nextId(counter, "cmd-proj"),
    projectId,
    title,
    workspaceRoot: harness.workspaceDir,
  });
  return projectId;
}

async function pairScopedWsClient(
  harness: WorkflowBoardE2eHarness,
  binaryPath: string,
  label: string,
  scopes: ReadonlyArray<string>,
): Promise<WorkflowBoardRpcClient> {
  const rawOut = NodeChildProcess.execFileSync(
    binaryPath,
    [
      "pair",
      "--base-dir",
      harness.homeDir,
      "--json",
      "--label",
      label,
      "--scope",
      scopes.join(","),
    ],
    {
      cwd: harness.workspaceDir,
      env: {
        ...process.env,
        HOME: harness.homeDir,
        USERPROFILE: harness.homeDir,
        LOCALAPPDATA: harness.homeDir,
      },
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const parsedPair = JSON.parse(rawOut) as { readonly token?: string };
  if (!parsedPair.token) {
    throw new Error("Missing pairing token from legioncode pair --json");
  }
  const pairRes = await fetch(`${harness.origin}/api/auth/pair`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token: parsedPair.token, clientLabel: label }),
  });
  if (pairRes.status !== 200) {
    throw new Error(`/api/auth/pair failed with status ${pairRes.status}`);
  }
  const pairBody = (await pairRes.json()) as { readonly accessToken?: string };
  if (!pairBody.accessToken) {
    throw new Error("Missing accessToken from /api/auth/pair");
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

async function mintAgentWsClient(
  harness: WorkflowBoardE2eHarness,
  subject: string,
  scopes: ReadonlyArray<string>,
): Promise<WorkflowBoardRpcClient> {
  const sessionRes = await fetch(`${harness.origin}/api/auth/sessions`, {
    method: "POST",
    headers: {
      cookie: harness.cookiePair,
      "content-type": "application/json",
    },
    body: JSON.stringify({ subject, label: subject, scopes: [...scopes] }),
  });
  if (sessionRes.status !== 201) {
    throw new Error(`/api/auth/sessions failed with status ${sessionRes.status}`);
  }
  const sessionBody = (await sessionRes.json()) as {
    readonly token?: string;
    readonly subject?: string;
  };
  if (!sessionBody.token || sessionBody.subject !== subject) {
    throw new Error(`Expected issued session for ${subject}`);
  }
  const wsOrigin = harness.origin.replace(/^http/, "ws");
  const ws = new NodeWS.WebSocket(`${wsOrigin}/ws?orchestrationProtocol=2&frameParts=1`, {
    headers: { authorization: `Bearer ${sessionBody.token}` },
  });
  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
  return new WorkflowBoardRpcClient(ws);
}

async function expectRpcFailure(
  fn: () => Promise<unknown>,
  expectedText?: string,
): Promise<string> {
  try {
    await fn();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    if (expectedText) {
      assert.include(message.toLowerCase(), expectedText.toLowerCase());
    }
    return message;
  }
  throw new Error("Expected RPC call to fail, but it succeeded");
}

function validSixLenses(includeUx = true) {
  return [
    includeUx
      ? { lens: "ux", applicable: true, summary: "4-column board and card detail UX." }
      : { lens: "ux", applicable: false, reason: "Backend-only feature without UI." },
    { lens: "functional", applicable: true, summary: "Revision-checked transitions." },
    { lens: "architecture", applicable: true, summary: "SQLite card ledger and WS RPCs." },
    { lens: "quality", applicable: true, summary: "Pairwise and E2E verification." },
    { lens: "infrastructure", applicable: true, summary: "Isolated git worktrees." },
    { lens: "delivery", applicable: true, summary: "Gated manual delivery receipt." },
  ];
}

function validTwoAlternatives() {
  return [
    {
      id: "alt-ledger",
      title: "SQLite revision ledger",
      summary: "Track immutable revisions and content-bound approvals.",
      tradeoffs: "Explicit schema; clear invalidation cascade.",
    },
    {
      id: "alt-inline",
      title: "Single mutable row",
      summary: "Overwrite state in place with JSON snapshots.",
      tradeoffs: "Smaller schema; weaker audit history.",
    },
  ];
}

async function createAndApproveM1Spec(
  harness: WorkflowBoardE2eHarness,
  projectId: string,
  worktreePath: string,
  title: string,
  brief: string,
): Promise<WorkflowBoardApproveSpecResult> {
  const created = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.createCard", {
    projectId,
    title,
    brief,
    sourceType: "idea",
    assistanceMode: "balanced",
    workspaceStrategy: { type: "existing_worktree", worktreePath },
    modelSelection: { instanceId: "devReplay", model: "dev-replay" },
    runtimeMode: "approval-required",
    interactionMode: "default",
  });
  const updated = await harness.rpc.callUnary<WorkflowBoardUpdateSpecResult>(
    "workflowBoard.updateSpec",
    {
      cardId: created.cardId,
      expectedRevision: created.revision,
      acceptanceCriteria: [
        { id: "AC-01", text: "Deliverables persist in the assigned worktree." },
        { id: "AC-02", text: "Approvals bind to the approved spec digest." },
      ],
      constraints: ["Never auto-commit deliverables"],
      sourceRefs: ["README.md"],
      wireframesIndex: null,
    },
  );
  return await harness.rpc.callUnary<WorkflowBoardApproveSpecResult>("workflowBoard.approveSpec", {
    cardId: created.cardId,
    expectedRevision: updated.revision,
  });
}

async function completeM2Plan(
  harness: WorkflowBoardE2eHarness,
  cardId: string,
  revision: number,
  options?: {
    readonly includeUx?: boolean;
    readonly strategy?: "outcomes" | "risk-first" | "expand-contract" | "custom";
    readonly granularity?: "coarse" | "balanced" | "fine";
    readonly filesAllowed?: ReadonlyArray<string>;
    readonly filesForbidden?: ReadonlyArray<string>;
    readonly checkCommand?: ReadonlyArray<string>;
  },
): Promise<number> {
  const checkCommand = options?.checkCommand ?? [process.execPath, "-e", "process.exit(0)"];
  const strategy = options?.strategy ?? "outcomes";
  const updatedPlan = await harness.rpc.callUnary<{ readonly revision: number }>(
    "workflowBoard.updatePlan",
    {
      cardId,
      expectedRevision: revision,
      lenses: validSixLenses(options?.includeUx ?? true),
      alternatives: validTwoAlternatives(),
      selectedAlternativeId: "alt-ledger",
      selectionRationale: "Revision ledger satisfies durability and invalidation requirements.",
      strategy,
      ...(strategy === "risk-first"
        ? {
            strategyMetadata: {
              uncertainty: "Verify risk probe",
              probeTaskId: "TSK-0001",
            },
          }
        : {}),
      granularity: options?.granularity ?? "balanced",
      tasks: [
        {
          taskId: "TSK-0001",
          title: "Implement scoped module",
          dependsOn: [],
          filesAllowed: options?.filesAllowed ?? ["src/feature.ts"],
          filesForbidden: options?.filesForbidden ?? ["src/forbidden.ts"],
          testingMethod: "test-first",
        },
      ],
      checks: [{ id: "CHK-1", criterionId: "AC-01", argv: checkCommand }],
      acMappings: [
        {
          criterionId: "AC-01",
          method: "automated",
          checkIds: ["CHK-1"],
          command: checkCommand,
        },
        {
          criterionId: "AC-02",
          method: "manual",
          procedure: "Inspect evidence summary and sign off.",
        },
      ],
    },
  );
  const approvedPlan = await harness.rpc.callUnary<{ readonly revision: number }>(
    "workflowBoard.approvePlan",
    { cardId, expectedRevision: updatedPlan.revision },
  );
  return approvedPlan.revision;
}

describe("E2E-M5 Tier 3 pairwise combinations (server TS)", () => {
  it.live(
    "[T3-S-01] F1 x F20: compact subscribe stream and worktree-scoped deliverables survive server restart",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s01", "T3 S01");
      const wt = harness.git!.createWorktree("feat/t3-s01");
      const headBefore = runGit(wt, ["rev-parse", "HEAD"]);

      const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
      try {
        const initialSnap = (await sub.nextChunk()) as ProjectWorkflowBoardSnapshot;
        assert.isArray(initialSnap.cards);

        const largeBrief = "Compact stream check ".repeat(300);
        const created = await harness.rpc.callUnary<WorkflowCardDetail>(
          "workflowBoard.createCard",
          {
            projectId,
            title: "Worktree Compact Card",
            brief: largeBrief,
            sourceType: "idea",
            assistanceMode: "balanced",
            workspaceStrategy: { type: "existing_worktree", worktreePath: wt },
          },
        );

        const nextSnap = (await sub.nextChunk((chunk) =>
          JSON.stringify(chunk).includes(created.cardId),
        )) as ProjectWorkflowBoardSnapshot;
        assert.strictEqual(nextSnap.cards.length, 1);
        assert.strictEqual(nextSnap.cards[0]?.cardId, created.cardId);
        assert.notInclude(JSON.stringify(nextSnap), largeBrief);

        const wtCardDir = NodePath.join(wt, ".legion", "workflows", created.cardId);
        assert.isTrue(NodeFS.existsSync(NodePath.join(wtCardDir, "card.json")));
        assert.isTrue(NodeFS.existsSync(NodePath.join(wtCardDir, "spec.md")));
        assert.isFalse(
          NodeFS.existsSync(
            NodePath.join(harness.workspaceDir, ".legion", "workflows", created.cardId),
          ),
        );
        assert.strictEqual(runGit(wt, ["rev-parse", "HEAD"]), headBefore);
      } finally {
        await sub.interrupt();
      }

      await harness.restart();
      const postSub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
      try {
        const postSnap = (await postSub.nextChunk()) as ProjectWorkflowBoardSnapshot;
        assert.strictEqual(postSnap.cards.length, 1);
        assert.strictEqual(postSnap.cards[0]?.title, "Worktree Compact Card");
      } finally {
        await postSub.interrupt();
      }
    }),
  );

  it.live(
    "[T3-S-02] F2 x F4: file brief intake and updateSpec populate detail and spec artifact without auto-commit",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s02", "T3 S02");
      harness.git!.commitFile(
        harness.workspaceDir,
        "docs/feature-brief.md",
        "# File Intake Brief\n\nImplement deterministic parser without UI.\n",
        "add brief",
      );
      const headBefore = runGit(harness.workspaceDir, ["rev-parse", "HEAD"]);

      const created = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.createCard", {
        projectId,
        title: "File Intake Card",
        briefPath: "docs/feature-brief.md",
        sourceType: "file",
        assistanceMode: "direct",
      });
      assert.strictEqual(created.spec.sourceType, "file");
      assert.strictEqual(created.spec.assistanceMode, "direct");
      assert.include(created.spec.brief, "File Intake Brief");

      const updated = await harness.rpc.callUnary<WorkflowBoardUpdateSpecResult>(
        "workflowBoard.updateSpec",
        {
          cardId: created.cardId,
          expectedRevision: created.revision,
          acceptanceCriteria: [{ id: "AC-1", text: "Parser rejects invalid frames." }],
          constraints: ["Zero side-effect commits"],
          sourceRefs: ["docs/feature-brief.md"],
          wireframesIndex: null,
        },
      );
      assert.isAbove(updated.revision, created.revision);

      const detail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: created.cardId,
      });
      assert.strictEqual(detail.spec.wireframesIndex, null);
      assert.strictEqual(detail.spec.acceptanceCriteria[0]?.id, "AC-1");

      const specArtifact = await harness.rpc.callUnary<WorkflowBoardArtifact>(
        "workflowBoard.getArtifact",
        { cardId: created.cardId, kind: "spec" },
      );
      assert.include(specArtifact.content, "AC-1");
      assert.include(specArtifact.content, "Zero side-effect commits");
      assert.strictEqual(runGit(harness.workspaceDir, ["rev-parse", "HEAD"]), headBefore);
    }),
  );

  it.live(
    "[T3-S-03] F8 x F20: external spec.md edit invalidates specApproval and blocks startStage('plan')",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s03", "T3 S03");
      const wt = harness.git!.createWorktree("feat/t3-s03");
      await harness.rpc.callUnary("server.updateSettings", { patch: { workflowsEnabled: true } });

      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Digest Reconciliation Card",
        "Verify external edit detection.",
      );
      assert.strictEqual(approved.specApproval.status, "valid");

      const specPath = NodePath.join(wt, ".legion", "workflows", approved.cardId, "spec.md");
      const original = NodeFS.readFileSync(specPath, "utf8");
      NodeFS.writeFileSync(specPath, `${original}\n<!-- external disk tamper -->\n`, "utf8");

      const reconciled = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(reconciled.specApprovalStatus, "stale");
      assert.strictEqual(reconciled.specApproval?.status, "stale");

      await expectRpcFailure(
        () =>
          harness.rpc.callUnary("workflowBoard.startStage", {
            cardId: approved.cardId,
            expectedRevision: reconciled.revision,
            stage: "plan",
          }),
        "spec_not_approved",
      );
    }),
  );

  it.live(
    "[T3-S-04] F13 x F21: startStage('plan') stages plan-brief.md and idempotently reuses Replay thread across restart",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s04", "T3 S04");
      const wt = harness.git!.createWorktree("feat/t3-s04");
      await harness.rpc.callUnary("server.updateSettings", { patch: { workflowsEnabled: true } });

      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Plan Brief Launch Card",
        "Verify brief assembly and launch idempotency.",
      );

      const firstLaunch = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: approved.revision,
          stage: "plan",
          stableLaunchKey: "t3-s04-stable-plan-key",
        },
      );
      assert.isFalse(firstLaunch.resumed);
      assert.strictEqual(firstLaunch.stage, "plan");
      assert.isString(firstLaunch.threadId);

      const briefArtifact = await harness.rpc.callUnary<WorkflowBoardArtifact>(
        "workflowBoard.getArtifact",
        { cardId: approved.cardId, key: "plan-brief.md" },
      );
      assert.include(briefArtifact.content, "AC-01");
      assert.include(briefArtifact.content, "Never auto-commit deliverables");
      assert.include(briefArtifact.content, "README.md");

      const threadProj = (await harness.rpc.callUnary("orchestration.getThreadProjection", {
        threadId: firstLaunch.threadId,
      })) as { readonly thread?: { readonly worktreePath?: string | null } };
      assert.strictEqual(threadProj.thread?.worktreePath, wt);

      await harness.restart();
      const postRestartDetail = await harness.rpc.callUnary<WorkflowCardDetail>(
        "workflowBoard.getCard",
        { cardId: approved.cardId },
      );

      const secondLaunch = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: postRestartDetail.revision,
          stage: "plan",
          stableLaunchKey: "t3-s04-stable-plan-key",
        },
      );
      assert.isTrue(secondLaunch.resumed);
      assert.strictEqual(secondLaunch.threadId, firstLaunch.threadId);
    }),
  );

  it.live(
    "[T3-S-05] F22 x F8: agent thread token can updateSpec but is denied client:interactive approveSpec",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s05", "T3 S05");
      const created = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.createCard", {
        projectId,
        title: "Agent Scope Guard Card",
        brief: "Agent proposes spec edits; human approves.",
      });

      const agentClient = await mintAgentWsClient(harness, "thread:thr-agent-s05", [
        "orchestration:read",
        "orchestration:operate",
      ]);
      try {
        const agentUpdate = await agentClient.callUnary<WorkflowBoardUpdateSpecResult>(
          "workflowBoard.updateSpec",
          {
            cardId: created.cardId,
            expectedRevision: created.revision,
            acceptanceCriteria: [{ id: "AC-1", text: "Proposed by agent session." }],
          },
        );
        assert.isAbove(agentUpdate.revision, created.revision);

        const deniedMsg = await expectRpcFailure(
          () =>
            agentClient.callUnary("workflowBoard.approveSpec", {
              cardId: created.cardId,
              expectedRevision: agentUpdate.revision,
            }),
          "EnvironmentAuthorizationError",
        );
        assert.include(deniedMsg, "client:interactive");
      } finally {
        agentClient.close();
      }

      const after = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: created.cardId,
      });
      assert.strictEqual(after.specApproval, null);
      assert.strictEqual(after.specApprovalStatus, null);
    }),
  );

  it.live(
    "[T3-S-06] F5 x F7: paused guided exploration answers feed bounded challenge synthesis verbatim",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s06", "T3 S06");
      const created = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.createCard", {
        projectId,
        title: "Explore Challenge Pair",
        brief: "Guided spec authoring with pause/resume.",
        sourceType: "idea",
        assistanceMode: "guided",
      });
      const paused = await harness.rpc.callUnary<WorkflowBoardExploreStepResult>(
        "workflowBoard.exploreStep",
        {
          cardId: created.cardId,
          expectedRevision: created.revision,
          action: "pause",
          answers: [{ decisionId: "D-1", choice: "recommend", note: "Verbatim operator note." }],
        },
      );
      assert.isTrue(paused.paused);
      const resumed = await harness.rpc.callUnary<WorkflowBoardExploreStepResult>(
        "workflowBoard.exploreStep",
        { cardId: created.cardId, expectedRevision: paused.revision, action: "resume" },
      );
      assert.isFalse(resumed.paused);
      assert.isAtMost(resumed.decisions.length, 3);
      assert.strictEqual(
        resumed.decisions.find((decision) => decision.id === "D-1")?.answer,
        "Verbatim operator note.",
      );

      const challenged = await harness.rpc.callUnary<WorkflowBoardChallengeStepResult>(
        "workflowBoard.challengeStep",
        {
          cardId: created.cardId,
          expectedRevision: resumed.revision,
          action: "synthesize",
          concerns: [
            {
              id: "C-1",
              summary: "Preserve the operator's decision.",
              disposition: "answer",
              answer: "Keep verbatim note.",
            },
          ],
        },
      );
      assert.lengthOf(challenged.concerns, 1);
      assert.strictEqual(challenged.challengeReceipt.synthesisCount, 1);
      const detail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: created.cardId,
      });
      assert.strictEqual(
        detail.challengeReceipt?.concerns.find((concern) => concern.id === "C-1")?.answer,
        "Keep verbatim note.",
      );
    }),
  );

  it.live(
    "[T3-S-07] F6 x F8: wireframe artifact binds into specApproval and edit invalidates approval",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s07", "T3 S07");
      const wt = harness.git!.createWorktree("feat/t3-s07");
      const nonUi = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.createCard", {
        projectId,
        title: "Non-UI Wireframe Null",
        brief: "Backend endpoint only.",
      });
      assert.strictEqual(nonUi.spec.wireframesIndex, null);

      const uiCard = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.createCard", {
        projectId,
        title: "UI Wireframe Bound",
        brief: "Interactive dashboard.",
        workspaceStrategy: { type: "existing_worktree", worktreePath: wt },
      });
      const wfPath = NodePath.join(
        wt,
        ".legion",
        "workflows",
        uiCard.cardId,
        "wireframes",
        "index.html",
      );
      NodeFS.mkdirSync(NodePath.dirname(wfPath), { recursive: true });
      NodeFS.writeFileSync(
        wfPath,
        "<!doctype html><html><body><h1>Wireframe v1</h1></body></html>\n",
        "utf8",
      );

      const updated = await harness.rpc.callUnary<WorkflowBoardUpdateSpecResult>(
        "workflowBoard.updateSpec",
        {
          cardId: uiCard.cardId,
          expectedRevision: uiCard.revision,
          acceptanceCriteria: [
            { id: "AC-01", text: "Render wireframe preview." },
            { id: "AC-02", text: "Inspect visual layout." },
          ],
          wireframesIndex: "wireframes/index.html",
        },
      );
      const approved = await harness.rpc.callUnary<WorkflowBoardApproveSpecResult>(
        "workflowBoard.approveSpec",
        { cardId: uiCard.cardId, expectedRevision: updated.revision },
      );
      assert.strictEqual(approved.specApproval.status, "valid");
      assert.isString(approved.specApproval.wireframesDigest);

      NodeFS.writeFileSync(
        wfPath,
        "<!doctype html><html><body><h1>Wireframe v2</h1></body></html>\n",
        "utf8",
      );
      const afterWfEdit = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: uiCard.cardId,
      });
      assert.strictEqual(afterWfEdit.specApprovalStatus, "stale");
      assert.strictEqual(afterWfEdit.specApproval?.status, "stale");
    }),
  );

  it.live(
    "[T3-S-08] F9 x F10: six-lens applicability pairs with 2-alternative comparison and rationale",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s08", "T3 S08");
      const wt = harness.git!.createWorktree("feat/t3-s08");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Lenses And Alternatives",
        "Backend design lenses.",
      );
      const rev = await completeM2Plan(harness, approved.cardId, approved.revision, {
        includeUx: false,
      });
      assert.isAbove(rev, approved.revision);
      const detail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      const serialized = JSON.stringify(detail);
      assert.include(serialized, "alt-ledger");
      assert.include(serialized, "Revision ledger satisfies");
    }),
  );

  it.live(
    "[T3-S-09] F11 x F12: risk-first fine plan maps all ACs and plan-only edit preserves specApproval",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s09", "T3 S09");
      const wt = harness.git!.createWorktree("feat/t3-s09");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Plan Only Edit Preserves Spec",
        "Strategy + AC mapping.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision, {
        strategy: "risk-first",
        granularity: "fine",
      });
      const planEdit = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.updatePlan",
        {
          cardId: approved.cardId,
          expectedRevision: planRev,
          strategy: "expand-contract",
          strategyMetadata: {
            compatibilityWindow: "Keep both versions during the migration",
            transitionStep: "Switch readers after the backfill",
            retirementPrereq: "All readers have switched",
          },
        },
      );
      assert.isAbove(planEdit.revision, planRev);
      const detail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(detail.specApprovalStatus, "valid");
    }),
  );

  it.live(
    "[T3-S-10] F14 x F16: single mutating task and argv check bind to snapshot and stale on snapshot edit",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s10", "T3 S10");
      const wt = harness.git!.createWorktree("feat/t3-s10");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Snapshot Bound Check Card",
        "Execute and validate snapshot.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision);
      const exec = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        { cardId: approved.cardId, expectedRevision: planRev, stage: "execute" },
      );
      assert.strictEqual(exec.stage, "execute");
      const executed = await settleStage(harness.rpc, exec);
      assert.strictEqual(
        executed.taskExecutions?.find((task) => task.taskId === "TSK-0001")?.status,
        "completed",
      );
      assert.lengthOf(executed.taskExecutions ?? [], 1);

      const beforeEdit = await runCheck(harness.rpc, executed);
      assert.strictEqual(beforeEdit.checkReceipts?.[0]?.status, "passed");
      assert.isString(beforeEdit.checkReceipts?.[0]?.productSnapshotDigest);

      harness.git!.commitFile(wt, "src/feature.ts", "export const v = 2;\n", "snapshot bump");
      const afterEdit = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(afterEdit.cardId, approved.cardId);
      assert.strictEqual(afterEdit.checkReceipts?.[0]?.status, "stale");
      assert.notStrictEqual(afterEdit.status, "completed");
    }),
  );

  it.live(
    "[T3-S-11] F15 x F17: pause/resume/retry pairs with fresh-thread review and finding disposition",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s11", "T3 S11");
      const wt = harness.git!.createWorktree("feat/t3-s11");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Pause Review Fresh Thread",
        "Exercise pause/resume and fresh review thread.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision, {
        checkCommand: checkScript(harness, "ready.txt"),
      });
      const execStart = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        { cardId: approved.cardId, expectedRevision: planRev, stage: "execute" },
      );
      const executed = await settleStage(harness.rpc, execStart);
      const paused = await harness.rpc.callUnary<{
        readonly revision: number;
        readonly status: string;
      }>("workflowBoard.pauseCard", {
        cardId: approved.cardId,
        expectedRevision: executed.revision,
      });
      assert.strictEqual(paused.status, "paused");
      const resumed = await harness.rpc.callUnary<WorkflowBoardResumeCardResult>(
        "workflowBoard.resumeCard",
        { cardId: approved.cardId, expectedRevision: paused.revision },
      );
      assert.notStrictEqual(resumed.card.status, "paused");
      const failed = await runCheck(harness.rpc, resumed.card);
      assert.strictEqual(failed.checkReceipts?.[0]?.status, "failed");
      NodeFS.writeFileSync(NodePath.join(wt, "ready.txt"), "ok\n", "utf8");
      const checked = await runCheck(harness.rpc, failed);
      assert.strictEqual(checked.checkReceipts?.[0]?.status, "passed");

      const reviewStart = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: checked.revision,
          stage: "review",
          modelSelection: await installReviewer(harness, true),
        },
      );
      assert.notStrictEqual(reviewStart.threadId, execStart.threadId);
      const reviewed = await settleStage(harness.rpc, reviewStart);
      assert.strictEqual(reviewed.reviewReport?.threadId, reviewStart.threadId);
      assert.strictEqual(reviewed.reviewReport?.verdict, "findings");
      assert.strictEqual(reviewed.reviewReport?.findings[0]?.id, "FIND-1");
      const resolved = await harness.rpc.callUnary<WorkflowBoardResolveFindingResult>(
        "workflowBoard.resolveFinding",
        {
          cardId: approved.cardId,
          expectedRevision: reviewed.revision,
          findingId: "FIND-1",
          disposition: "accept_risk",
          reason: "Documentation already covers the operator action.",
        },
      );
      assert.strictEqual(resolved.finding.disposition, "accept_risk");
      assert.strictEqual(resolved.finding.status, "resolved");
    }),
  );

  it.live(
    "[T3-S-12] F18 x F19: manual acceptance and approveDelivery never auto-commit before individual git commit",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t3-s12", "T3 S12");
      const wt = harness.git!.createWorktree("feat/t3-s12");
      const headBefore = runGit(wt, ["rev-parse", "HEAD"]);
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Ship Individual Git Card",
        "Verify Ship gate + individual commit.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision);
      const exec = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        { cardId: approved.cardId, expectedRevision: planRev, stage: "execute" },
      );
      const executed = await settleStage(harness.rpc, exec);
      const checked = await runCheck(harness.rpc, executed);
      assert.strictEqual(checked.checkReceipts?.[0]?.status, "passed");

      const review = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: checked.revision,
          stage: "review",
          modelSelection: await installReviewer(harness),
        },
      );
      assert.notStrictEqual(review.threadId, exec.threadId);
      const afterReview = await settleStage(harness.rpc, review);
      assert.strictEqual(afterReview.reviewReport?.verdict, "pass");

      const accepted = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.recordManualAcceptance",
        {
          cardId: approved.cardId,
          expectedRevision: afterReview.revision,
          criterionId: "AC-02",
          verdict: "pass",
          note: "Verified manually.",
        },
      );
      await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: approved.cardId,
        expectedRevision: accepted.revision,
      });
      assert.strictEqual(runGit(wt, ["rev-parse", "HEAD"]), headBefore);

      NodeFS.writeFileSync(NodePath.join(wt, "shipped.txt"), "ready to commit\n", "utf8");
      const sub = harness.rpc.subscribe("git.runStackedAction", {
        actionId: "t3-s12-commit",
        cwd: wt,
        action: "commit",
        commitMessage: "feat: ship card individually",
      });
      try {
        while (true) {
          const chunk = (await sub.nextChunk()) as {
            readonly kind?: string;
            readonly result?: {
              readonly commit?: { readonly status?: string };
              readonly push?: { readonly status?: string };
              readonly pr?: { readonly status?: string };
            };
          };
          assert.notStrictEqual(chunk.kind, "action_failed");
          if (chunk.kind === "action_finished") {
            assert.strictEqual(chunk.result?.commit?.status, "created");
            assert.strictEqual(chunk.result?.push?.status, "skipped_not_requested");
            assert.strictEqual(chunk.result?.pr?.status, "skipped_not_requested");
            break;
          }
        }
      } finally {
        await sub.interrupt();
      }
      assert.notStrictEqual(runGit(wt, ["rev-parse", "HEAD"]), headBefore);
    }),
  );
});

describe("E2E-M5 Tier 4 real-world scenarios (server TS)", () => {
  it.live(
    "[T4-S-01] Scenario 1: M1 one-handoff across host restart with Replay thread in explicit worktree",
    runScenarioTest(async (harness, binaryPath) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s01", "Scenario 1");
      const wt = harness.git!.createWorktree("feat/t4-s01");
      const headBefore = runGit(wt, ["rev-parse", "HEAD"]);

      await harness.rpc.callUnary("server.updateSettings", {
        patch: { workflowsEnabled: true },
      });

      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "M1 Handoff Feature",
        "Prove M1 handoff from Spec approval to Plan brief across host restart.",
      );
      assert.strictEqual(approved.specApproval.status, "valid");
      const approvedDigest = approved.specApproval.specDigest;
      assert.isString(approvedDigest);

      await harness.restart();

      const clientB = await pairScopedWsClient(harness, binaryPath, "scenario-1-client-b", [
        "orchestration:read",
        "orchestration:operate",
      ]);
      try {
        const cardAfterRestart = await clientB.callUnary<WorkflowCardDetail>(
          "workflowBoard.getCard",
          { cardId: approved.cardId },
        );
        assert.strictEqual(cardAfterRestart.specApprovalStatus, "valid");
        assert.strictEqual(cardAfterRestart.specApproval?.specDigest, approvedDigest);

        const firstPlan = await clientB.callUnary<WorkflowBoardStartStageResult>(
          "workflowBoard.startStage",
          {
            cardId: approved.cardId,
            expectedRevision: cardAfterRestart.revision,
            stage: "plan",
          },
        );
        assert.isFalse(firstPlan.resumed);
        assert.strictEqual(firstPlan.stage, "plan");

        const secondPlan = await clientB.callUnary<WorkflowBoardStartStageResult>(
          "workflowBoard.startStage",
          {
            cardId: approved.cardId,
            expectedRevision: firstPlan.revision,
            stage: "plan",
          },
        );
        assert.isTrue(secondPlan.resumed);
        assert.strictEqual(secondPlan.threadId, firstPlan.threadId);

        const threadProj = (await clientB.callUnary("orchestration.getThreadProjection", {
          threadId: firstPlan.threadId,
        })) as {
          readonly thread?: {
            readonly projectId?: string;
            readonly worktreePath?: string | null;
          };
        };
        assert.strictEqual(threadProj.thread?.projectId, projectId);
        assert.strictEqual(threadProj.thread?.worktreePath, wt);

        const planBrief = await clientB.callUnary<WorkflowBoardArtifact>(
          "workflowBoard.getArtifact",
          { cardId: approved.cardId, key: "plan-brief.md" },
        );
        assert.include(planBrief.content, "AC-01");
        assert.strictEqual(runGit(wt, ["rev-parse", "HEAD"]), headBefore);
      } finally {
        clientB.close();
      }
    }),
  );

  it.live(
    "[T4-S-04] Scenario 4: Spec revision return and targeted invalidation cascade",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s04", "Scenario 4");
      const wt = harness.git!.createWorktree("feat/t4-s04");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Invalidation Cascade",
        "Test spec edit vs plan-only edit.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision);

      const returned = await harness.rpc.callUnary<{
        readonly revision: number;
        readonly stage: string;
      }>("workflowBoard.returnForRevision", {
        cardId: approved.cardId,
        expectedRevision: planRev,
        targetStage: "spec",
        reason: "Clarify constraint before execute",
      });
      assert.strictEqual(returned.stage, "spec");

      const specEdit = await harness.rpc.callUnary<WorkflowBoardUpdateSpecResult>(
        "workflowBoard.updateSpec",
        {
          cardId: approved.cardId,
          expectedRevision: returned.revision,
          brief: "Updated spec brief invalidating both Spec and Plan.",
        },
      );
      assert.strictEqual(specEdit.specApprovalStatus, "stale");
      assert.strictEqual(specEdit.card.planApproval?.status, "stale");
      await expectRpcFailure(
        () =>
          harness.rpc.callUnary("workflowBoard.startStage", {
            cardId: approved.cardId,
            expectedRevision: specEdit.revision,
            stage: "execute",
          }),
        "invalid_stage_transition",
      );

      const reapprovedSpec = await harness.rpc.callUnary<WorkflowBoardApproveSpecResult>(
        "workflowBoard.approveSpec",
        { cardId: approved.cardId, expectedRevision: specEdit.revision },
      );
      const planOnlyEdit = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.updatePlan",
        {
          cardId: approved.cardId,
          expectedRevision: reapprovedSpec.revision,
          selectionRationale: "Plan-only adjustment keeps Spec valid.",
        },
      );
      assert.isAbove(planOnlyEdit.revision, reapprovedSpec.revision);
      const detail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(detail.specApprovalStatus, "valid");
      assert.isAbove(detail.revisions.length, 2);
    }),
  );

  it.live(
    "[T4-S-05] Scenario 5: Out-of-contract turn edits preserved and unblocked via Plan amendment",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s05", "Scenario 5");
      const wt = harness.git!.createWorktree("feat/t4-s05");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Out-of-Contract Amendment",
        "Verify unexpected edits are preserved and resolved via amendment.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision);
      const extraPath = NodePath.join(wt, "src", "extra.ts");
      const modelSelection = await installWorkflowBoardTestProvider(
        harness,
        "scenario-executor",
        "Completed the task.",
        [
          "const fs = await import('node:fs');",
          "fs.mkdirSync('src', { recursive: true });",
          "fs.writeFileSync('src/extra.ts', 'export const extra = 1;\\n');",
        ].join("\n"),
      );
      const exec = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        { cardId: approved.cardId, expectedRevision: planRev, stage: "execute", modelSelection },
      );
      const preAmend = await settleStage(harness.rpc, exec);
      assert.strictEqual(preAmend.status, "blocked");
      assert.include(preAmend.fileContractReports?.[0]?.unexpectedFiles ?? [], "src/extra.ts");
      assert.isTrue(preAmend.fileContractReports?.[0]?.preservedInWorktree);
      assert.strictEqual(NodeFS.readFileSync(extraPath, "utf8"), "export const extra = 1;\n");

      const amended = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.proposeAmendment",
        {
          cardId: approved.cardId,
          expectedRevision: preAmend.revision,
          targetStage: "plan",
          reason: "Include src/extra.ts in allowed contract",
          sourceTaskId: "TSK-0001",
          unexpectedFiles: ["src/extra.ts"],
          tasks: [
            {
              taskId: "TSK-0001",
              title: "Implement scoped module",
              dependsOn: [],
              filesAllowed: ["src/feature.ts", "src/extra.ts"],
              filesForbidden: ["src/forbidden.ts"],
              testingMethod: "test-first",
            },
          ],
        },
      );
      assert.isAbove(amended.revision, preAmend.revision);

      const reapprovedPlan = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.approvePlan",
        { cardId: approved.cardId, expectedRevision: amended.revision },
      );
      let nextRev = reapprovedPlan.revision;
      const afterPlan = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      if (afterPlan.status === "paused" || afterPlan.status === "blocked") {
        const resumed = await harness.rpc.callUnary<{ readonly revision: number }>(
          "workflowBoard.resumeCard",
          { cardId: approved.cardId, expectedRevision: afterPlan.revision },
        );
        nextRev = resumed.revision;
      }

      const exec2 = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        { cardId: approved.cardId, expectedRevision: nextRev, stage: "execute" },
      );
      const executed = await settleStage(harness.rpc, exec2);
      const checked = await runCheck(harness.rpc, executed);
      assert.strictEqual(checked.checkReceipts?.[0]?.status, "passed");
      const review = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: checked.revision,
          stage: "review",
          modelSelection: await installReviewer(harness),
        },
      );
      assert.notStrictEqual(review.threadId, exec2.threadId);
      const afterReview = await settleStage(harness.rpc, review);
      assert.strictEqual(afterReview.reviewReport?.verdict, "pass");
      const accepted = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.recordManualAcceptance",
        {
          cardId: approved.cardId,
          expectedRevision: afterReview.revision,
          criterionId: "AC-02",
          verdict: "pass",
          note: "Amendment scope verified.",
        },
      );
      await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: approved.cardId,
        expectedRevision: accepted.revision,
      });

      const finalDetail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(finalDetail.stage, "ship");
      assert.strictEqual(finalDetail.status, "completed");
      assert.strictEqual(NodeFS.readFileSync(extraPath, "utf8"), "export const extra = 1;\n");
    }),
  );

  it.live(
    "[T4-S-06] Scenario 6: Failed check -> snapshot change stales evidence -> repair -> ship",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s06", "Scenario 6");
      const wt = harness.git!.createWorktree("feat/t4-s06");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Failed Check Repair Card",
        "Verify check failure, snapshot stale cascade, and repair.",
      );
      const planRev = await completeM2Plan(harness, approved.cardId, approved.revision, {
        checkCommand: checkScript(harness, "ready.txt"),
      });
      const exec = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        { cardId: approved.cardId, expectedRevision: planRev, stage: "execute" },
      );
      const executed = await settleStage(harness.rpc, exec);
      assert.strictEqual(executed.taskExecutions?.[0]?.status, "completed");

      const blocked = await runCheck(harness.rpc, executed);
      assert.isTrue(
        blocked.shipReadiness?.blockers.some((blocker) => blocker.kind === "check_failed"),
      );
      assert.strictEqual(blocked.checkReceipts?.[0]?.status, "failed");
      assert.strictEqual(blocked.checkReceipts?.[0]?.exitCode, 1);
      assert.isString(blocked.checkReceipts?.[0]?.logRef);

      NodeFS.writeFileSync(NodePath.join(wt, "ready.txt"), "ok\n", "utf8");
      const checked = await runCheck(harness.rpc, blocked);
      assert.strictEqual(checked.checkReceipts?.[0]?.status, "passed");

      const reviewer = await installReviewer(harness);
      const review1 = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: checked.revision,
          stage: "review",
          modelSelection: reviewer,
        },
      );
      assert.notStrictEqual(review1.threadId, exec.threadId);
      const afterRev1 = await settleStage(harness.rpc, review1);
      assert.strictEqual(afterRev1.reviewReport?.verdict, "pass");
      const accepted1 = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.recordManualAcceptance",
        {
          cardId: approved.cardId,
          expectedRevision: afterRev1.revision,
          criterionId: "AC-02",
          verdict: "pass",
          note: "Initial manual acceptance before snapshot fix.",
        },
      );

      harness.git!.commitFile(wt, "src/fix.ts", "export const fixed = true;\n", "fix");
      const staleDetail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(staleDetail.checkReceipts?.[0]?.status, "stale");
      assert.strictEqual(staleDetail.reviewReport?.status, "stale");
      assert.strictEqual(
        staleDetail.acObservations?.find((criterion) => criterion.criterionId === "AC-02")?.status,
        "stale",
      );
      await expectRpcFailure(() =>
        harness.rpc.callUnary("workflowBoard.approveDelivery", {
          cardId: approved.cardId,
          expectedRevision: Math.max(accepted1.revision, staleDetail.revision),
        }),
      );

      const rechecked = await runCheck(harness.rpc, staleDetail);
      assert.strictEqual(rechecked.checkReceipts?.[0]?.status, "passed");
      const review2 = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: rechecked.revision,
          stage: "review",
          modelSelection: reviewer,
        },
      );
      assert.notStrictEqual(review2.threadId, review1.threadId);
      const afterRev2 = await settleStage(harness.rpc, review2);
      assert.strictEqual(afterRev2.reviewReport?.verdict, "pass");
      const accepted2 = await harness.rpc.callUnary<{ readonly revision: number }>(
        "workflowBoard.recordManualAcceptance",
        {
          cardId: approved.cardId,
          expectedRevision: afterRev2.revision,
          criterionId: "AC-02",
          verdict: "pass",
          note: "Verified on repaired snapshot.",
        },
      );
      await harness.rpc.callUnary("workflowBoard.approveDelivery", {
        cardId: approved.cardId,
        expectedRevision: accepted2.revision,
      });

      const shipped = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      assert.strictEqual(shipped.stage, "ship");
      assert.strictEqual(shipped.status, "completed");
    }),
  );

  it.live(
    "[T4-S-08] Scenario 8: Provider/model switch and retry maintain card identity and brief continuity",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s08", "Scenario 8");
      const wt = harness.git!.createWorktree("feat/t4-s08");
      const approved = await createAndApproveM1Spec(
        harness,
        projectId,
        wt,
        "Provider Switch Continuity",
        "Switch model selection across stages and retry failed step.",
      );
      const planStart = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: approved.revision,
          stage: "plan",
          modelSelection: { instanceId: "devReplay", model: "dev-replay-plan" },
          runtimeMode: "approval-required",
        },
      );
      await awaitPlanTurn(harness.rpc, planStart.threadId);
      const planBrief = await harness.rpc.callUnary<WorkflowBoardArtifact>(
        "workflowBoard.getArtifact",
        { cardId: approved.cardId, key: "plan-brief.md" },
      );
      assert.include(planBrief.content, "AC-01");
      assert.include(planBrief.content, "AC-02");
      assert.include(planBrief.content, "Never auto-commit deliverables");
      assert.include(planBrief.content, "README.md");

      const postPlan = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: approved.cardId,
      });
      const planRev = await completeM2Plan(harness, approved.cardId, postPlan.revision, {
        checkCommand: checkScript(harness, "step-fixed.txt"),
      });
      const exec = await harness.rpc.callUnary<WorkflowBoardStartStageResult>(
        "workflowBoard.startStage",
        {
          cardId: approved.cardId,
          expectedRevision: planRev,
          stage: "execute",
          modelSelection: { instanceId: "devReplay", model: "dev-replay-exec" },
          runtimeMode: "auto-accept-edits",
        },
      );
      const executed = await settleStage(harness.rpc, exec);

      const execProj = decodeThreadProjection(
        await harness.rpc.callUnary("orchestration.getThreadProjection", {
          threadId: exec.threadId,
        }),
      );
      assert.include(JSON.stringify(execProj), "AC-01");

      const failed = await runCheck(harness.rpc, executed);
      assert.strictEqual(failed.checkReceipts?.[0]?.status, "failed");
      assert.strictEqual(failed.taskExecutions?.[0]?.status, "completed");

      NodeFS.writeFileSync(NodePath.join(wt, "step-fixed.txt"), "fixed\n", "utf8");
      const detail = await runCheck(harness.rpc, failed);
      assert.isAbove(detail.revision, failed.revision);
      assert.strictEqual(detail.cardId, approved.cardId);
      assert.isAtLeast(detail.revisions.length, 3);
      assert.isAtLeast(detail.stageRuns.length, 2);
      const distinctModels = new Set(detail.stageRuns.map((sr) => sr.modelSelection.model));
      assert.isTrue(distinctModels.has("dev-replay-plan"));
      assert.isTrue(distinctModels.has("dev-replay-exec"));
      assert.strictEqual(detail.checkReceipts?.[0]?.status, "passed");
    }),
  );

  it.live(
    "[T4-S-09] Scenario 9: Disposable .legion-cli import produces 1 card, 8 tasks, blocked TSK-0002, and 0 duplicates on repeat",
    runScenarioTest(async (harness) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s09", "Scenario 9");
      harness.stageDisposableCliProject();
      assert.isFalse(
        NodeFS.existsSync(NodePath.join(harness.workspaceDir, ".legion-cli", "cache")),
      );
      const headBefore = runGit(harness.workspaceDir, ["rev-parse", "HEAD"]);

      const preview = await harness.rpc.callUnary<Record<string, unknown>>(
        "workflowBoard.previewCliImport",
        { projectId },
      );
      const previewJson = JSON.stringify(preview);
      assert.include(previewJson, "spec-t3-code");
      assert.include(previewJson, "TSK-0002");
      assert.include(previewJson, "blocked");
      assert.include(previewJson, "passScore");
      assert.include(previewJson, "requireHardened");
      assert.include(previewJson, "trustKeys");

      const firstApply = await harness.rpc.callUnary<{ readonly cardId: string }>(
        "workflowBoard.applyCliImport",
        { projectId },
      );
      const firstJson = JSON.stringify(firstApply);
      assert.include(firstJson, "TSK-0008");

      const detail = await harness.rpc.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
        cardId: firstApply.cardId,
      });
      assert.isArray(detail.tasks);
      assert.strictEqual(detail.tasks?.length, 8);
      const tsk2 = detail.tasks?.find((task) => task.taskId === "TSK-0002");
      assert.isDefined(tsk2);
      assert.strictEqual(tsk2?.status, "blocked");
      const detailJson = JSON.stringify(detail);
      assert.include(detailJson, "packages/provider-mimo/qualification/cli.test.mjs");
      assert.include(detailJson, "verify-1.log");
      assert.include(detailJson, "passScore");
      assert.include(detailJson, "requireHardened");
      assert.include(detailJson, "trustKeys");

      const repeatApply = await harness.rpc.callUnary<{ readonly cardId: string }>(
        "workflowBoard.applyCliImport",
        { projectId },
      );
      assert.strictEqual(repeatApply.cardId, firstApply.cardId);

      const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
      try {
        const snap = (await sub.nextChunk()) as ProjectWorkflowBoardSnapshot;
        assert.strictEqual(snap.cards.length, 1);
      } finally {
        await sub.interrupt();
      }
      const repeatDetail = await harness.rpc.callUnary<WorkflowCardDetail>(
        "workflowBoard.getCard",
        {
          cardId: firstApply.cardId,
        },
      );
      assert.strictEqual(repeatDetail.tasks?.length, 8);
      assert.strictEqual(runGit(harness.workspaceDir, ["rev-parse", "HEAD"]), headBefore);
    }),
  );

  it.live(
    "[T4-S-11] Scenario 11 (backend): Desktop client creates/executes, Mobile client approves, Web client reopens completed view",
    runScenarioTest(async (harness, binaryPath) => {
      const counter: Counter = { value: 0 };
      const projectId = await createProject(harness, counter, "proj-t4-s11", "Scenario 11");
      const wt = harness.git!.createWorktree("feat/t4-s11");
      await harness.rpc.callUnary("server.updateSettings", { patch: { workflowsEnabled: true } });

      const desktopClient = await pairScopedWsClient(harness, binaryPath, "desktop-e2e", [
        "orchestration:read",
        "orchestration:operate",
      ]);
      const mobileClient = await pairScopedWsClient(harness, binaryPath, "mobile-e2e", [
        "orchestration:read",
        "orchestration:operate",
      ]);
      const webClient = await pairScopedWsClient(harness, binaryPath, "web-e2e", [
        "orchestration:read",
        "orchestration:operate",
      ]);
      try {
        const created = await desktopClient.callUnary<WorkflowCardDetail>(
          "workflowBoard.createCard",
          {
            projectId,
            title: "Cross-Surface Card",
            brief: "Desktop -> Mobile -> Web shared host flow.",
            workspaceStrategy: { type: "existing_worktree", worktreePath: wt },
            modelSelection: { instanceId: "devReplay", model: "dev-replay" },
            runtimeMode: "approval-required",
            interactionMode: "default",
          },
        );
        const specUp = await desktopClient.callUnary<WorkflowBoardUpdateSpecResult>(
          "workflowBoard.updateSpec",
          {
            cardId: created.cardId,
            expectedRevision: created.revision,
            acceptanceCriteria: [
              { id: "AC-01", text: "Automated check passes in card worktree." },
              { id: "AC-02", text: "Multi-surface approval consistency." },
            ],
          },
        );
        const specOk = await desktopClient.callUnary<WorkflowBoardApproveSpecResult>(
          "workflowBoard.approveSpec",
          { cardId: created.cardId, expectedRevision: specUp.revision },
        );
        const planOkRev = await completeM2Plan(harness, created.cardId, specOk.revision);
        const execStarted = await desktopClient.callUnary<WorkflowBoardStartStageResult>(
          "workflowBoard.startStage",
          { cardId: created.cardId, expectedRevision: planOkRev, stage: "execute" },
        );
        const executed = await settleStage(desktopClient, execStarted);
        const checked = await runCheck(desktopClient, executed);
        assert.strictEqual(checked.checkReceipts?.[0]?.status, "passed");

        const mobileDetail = await mobileClient.callUnary<WorkflowCardDetail>(
          "workflowBoard.getCard",
          {
            cardId: created.cardId,
          },
        );
        assert.strictEqual(mobileDetail.cardId, created.cardId);
        assert.strictEqual(mobileDetail.revision, checked.revision);
        assert.strictEqual(mobileDetail.checkReceipts?.[0]?.status, "passed");
        const paused = await mobileClient.callUnary<{ readonly revision: number }>(
          "workflowBoard.pauseCard",
          { cardId: created.cardId, expectedRevision: mobileDetail.revision },
        );
        const resumed = await mobileClient.callUnary<{ readonly revision: number }>(
          "workflowBoard.resumeCard",
          { cardId: created.cardId, expectedRevision: paused.revision },
        );

        const reviewStarted = await desktopClient.callUnary<WorkflowBoardStartStageResult>(
          "workflowBoard.startStage",
          {
            cardId: created.cardId,
            expectedRevision: resumed.revision,
            stage: "review",
            modelSelection: await installReviewer(harness),
          },
        );
        assert.notStrictEqual(reviewStarted.threadId, execStarted.threadId);
        const reviewed = await settleStage(desktopClient, reviewStarted);
        assert.strictEqual(reviewed.reviewReport?.verdict, "pass");
        const postReview = await mobileClient.callUnary<WorkflowCardDetail>(
          "workflowBoard.getCard",
          {
            cardId: created.cardId,
          },
        );

        const accepted = await mobileClient.callUnary<{ readonly revision: number }>(
          "workflowBoard.recordManualAcceptance",
          {
            cardId: created.cardId,
            expectedRevision: postReview.revision,
            criterionId: "AC-02",
            verdict: "pass",
            note: "Mobile sign-off",
          },
        );
        await mobileClient.callUnary("workflowBoard.approveDelivery", {
          cardId: created.cardId,
          expectedRevision: accepted.revision,
        });

        const webDetail = await webClient.callUnary<WorkflowCardDetail>("workflowBoard.getCard", {
          cardId: created.cardId,
        });
        assert.strictEqual(webDetail.title, "Cross-Surface Card");
        assert.strictEqual(webDetail.stage, "ship");
        assert.strictEqual(webDetail.status, "completed");
      } finally {
        desktopClient.close();
        mobileClient.close();
        webClient.close();
      }
    }),
  );
});

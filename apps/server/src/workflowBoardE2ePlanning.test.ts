// @effect-diagnostics nodeBuiltinImport:off -- E2E planning tests inspect .legion/workflows/<card-id>/ files, git status, and isolated CLI processes.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

import * as NodeWS from "@effect/platform-node/NodeWS";
import { assert, it } from "@effect/vitest";
import { ServerConfig } from "@legioncode/contracts";
import * as HostProcess from "@legioncode/shared/HostProcess";
import * as Effect from "effect/Effect";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import { resolveRustCliBinary } from "./rustCutover.ts";
import {
  initIsolatedGitRepo,
  resolveWorkflowBoardRepoRoot,
  startWorkflowBoardE2eHarness,
  WorkflowBoardRpcClient,
  type WorkflowBoardE2eHarness,
} from "./workflowBoardE2eHarness.ts";

const decodeServerConfig = Schema.decodeUnknownSync(Schema.toCodecJson(ServerConfig));

interface PlanningCardFixture {
  readonly projectId: string;
  readonly cardId: string;
  readonly revision: number;
}

function runPlanningTest(
  run: (harness: WorkflowBoardE2eHarness, binaryPath: string) => Promise<void>,
) {
  return () =>
    Effect.gen(function* () {
      const repoRoot = resolveWorkflowBoardRepoRoot();
      const platform = yield* HostProcess.Platform;
      const env = yield* HostProcess.Environment;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env });
      assert.isDefined(binaryPath, "Expected built Rust legioncode binary");

      yield* Effect.promise(async () => {
        await using harness = await startWorkflowBoardE2eHarness({
          repoRoot,
          platform,
          binaryPath,
          seedGitWorkspace: true,
          requireBoardCapability: true,
        });
        assert.isTrue(
          harness.environment.projectWorkflowBoard,
          "Expected capabilities.projectWorkflowBoard === true",
        );
        await run(harness, binaryPath);
      });
    });
}

function runUnconditionalServerTest(
  run: (harness: WorkflowBoardE2eHarness, binaryPath: string) => Promise<void>,
) {
  return () =>
    Effect.gen(function* () {
      const repoRoot = resolveWorkflowBoardRepoRoot();
      const platform = yield* HostProcess.Platform;
      const env = yield* HostProcess.Environment;
      const binaryPath = resolveRustCliBinary({ repoRoot, platform, env });
      assert.isDefined(binaryPath, "Expected built Rust legioncode binary");

      yield* Effect.promise(async () => {
        await using harness = await startWorkflowBoardE2eHarness({
          repoRoot,
          platform,
          binaryPath,
          seedGitWorkspace: true,
        });
        await run(harness, binaryPath);
      });
    });
}

async function pairScopedWsClient(
  harness: WorkflowBoardE2eHarness,
  binaryPath: string,
  label: string,
  scopes: ReadonlyArray<string>,
): Promise<WorkflowBoardRpcClient> {
  const args = [
    "pair",
    "--base-dir",
    harness.homeDir,
    "--json",
    "--label",
    label,
    "--scope",
    scopes.join(","),
  ];
  const rawOut = NodeChildProcess.execFileSync(binaryPath, args, {
    cwd: harness.workspaceDir,
    env: {
      ...process.env,
      HOME: harness.homeDir,
      USERPROFILE: harness.homeDir,
      LOCALAPPDATA: harness.homeDir,
    },
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
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
    throw new Error(
      `Expected issued session for subject "${subject}", got ${JSON.stringify(sessionBody)}`,
    );
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

async function registerProject(
  harness: WorkflowBoardE2eHarness,
  projectId: string,
  title: string,
  workspaceRoot: string,
): Promise<string> {
  await harness.rpc.callUnary("orchestration.v2.dispatchCommand", {
    type: "project.create",
    commandId: `cmd-create-${projectId}`,
    projectId,
    title,
    workspaceRoot,
  });
  return projectId;
}

function validSixLenses(includeUx = true) {
  return [
    includeUx
      ? { lens: "ux", applicable: true, summary: "4-column board + split view UX." }
      : { lens: "ux", applicable: false, reason: "Backend-only service; no user interface." },
    { lens: "functional", applicable: true, summary: "CRUD + transition invariants." },
    { lens: "architecture", applicable: true, summary: "SQLite card state + WS RPC boundary." },
    { lens: "quality", applicable: true, summary: "Category-partition & boundary tests." },
    { lens: "infrastructure", applicable: true, summary: "Isolated worktree + temp SQLite." },
    { lens: "delivery", applicable: true, summary: "Incremental gated rollout." },
  ];
}

function validTwoAlternatives() {
  return [
    {
      id: "alt-a",
      title: "Event-sourced SQLite projection",
      summary: "Append card revisions with optimistic version check.",
      tradeoffs: "Explicit revision ledger; easy audit trail.",
    },
    {
      id: "alt-b",
      title: "Mutable snapshot row with journal",
      summary: "Maintain single row plus write-ahead artifact journal.",
      tradeoffs: "Simpler queries; requires explicit journal recovery.",
    },
  ];
}

function validDisjointTasks() {
  return [
    {
      taskId: "TSK-0001",
      title: "Implement core validator",
      dependsOn: [],
      filesAllowed: ["src/validator.ts"],
      filesForbidden: ["src/secrets.ts"],
      testingMethod: "test-first",
    },
    {
      taskId: "TSK-0002",
      title: "Wire RPC endpoint",
      dependsOn: ["TSK-0001"],
      filesAllowed: ["src/rpc.ts"],
      filesForbidden: ["src/validator.ts"],
      testingMethod: "regression-first",
    },
  ];
}

function validAcMappings() {
  return [
    {
      criterionId: "AC-01",
      method: "automated",
      command: ["node", "--test", "src/validator.test.ts"],
    },
    {
      criterionId: "AC-02",
      method: "manual",
      procedure: "Verify board overview card badge displays blocker reason clearly.",
    },
  ];
}

async function createSpecAndApprove(
  harness: WorkflowBoardE2eHarness,
  projectId: string,
  suffix: string,
  options?: { readonly withWireframes?: boolean },
): Promise<PlanningCardFixture> {
  const created = await harness.rpc.callUnary<{
    readonly cardId: string;
    readonly revision: number;
  }>("workflowBoard.createCard", {
    projectId,
    title: `Feature ${suffix}`,
    brief: `Deliver feature ${suffix} safely with isolated persistence.`,
    sourceType: "idea",
    assistanceMode: "balanced",
  });

  const updatedSpec = await harness.rpc.callUnary<{
    readonly revision: number;
    readonly specDigest?: string;
  }>("workflowBoard.updateSpec", {
    cardId: created.cardId,
    expectedRevision: created.revision,
    acceptanceCriteria: [
      { id: "AC-01", text: "Validation rejects malformed inputs." },
      { id: "AC-02", text: "Card state persists across restart." },
    ],
    constraints: ["No auto-commits during authoring"],
    sourceRefs: ["README.md"],
    wireframesIndex: options?.withWireframes ? "wireframes/index.html" : null,
  });

  const challenged = await harness.rpc.callUnary<{
    readonly revision: number;
  }>("workflowBoard.challengeStep", {
    cardId: created.cardId,
    expectedRevision: updatedSpec.revision,
    action: "complete",
    concerns: [],
  });

  const approved = await harness.rpc.callUnary<{
    readonly revision: number;
  }>("workflowBoard.approveSpec", {
    cardId: created.cardId,
    expectedRevision: challenged.revision,
  });

  return { projectId, cardId: created.cardId, revision: approved.revision };
}

async function populateAndApprovePlan(
  harness: WorkflowBoardE2eHarness,
  card: PlanningCardFixture,
): Promise<PlanningCardFixture> {
  const updatedPlan = await harness.rpc.callUnary<{
    readonly revision: number;
  }>("workflowBoard.updatePlan", {
    cardId: card.cardId,
    expectedRevision: card.revision,
    lenses: validSixLenses(true),
    alternatives: validTwoAlternatives(),
    selectedAlternativeId: "alt-a",
    selectionRationale: "Explicit revision ledger matches R2 durability requirements.",
    strategy: "outcomes",
    granularity: "balanced",
    tasks: validDisjointTasks(),
    acMappings: validAcMappings(),
  });

  const approvedPlan = await harness.rpc.callUnary<{
    readonly revision: number;
  }>("workflowBoard.approvePlan", {
    cardId: card.cardId,
    expectedRevision: updatedPlan.revision,
  });

  return { ...card, revision: approvedPlan.revision };
}

it.live(
  "[UNCOND-S-01] probes environment capabilities, toggles workflowsEnabled settings atomically, and keeps workflows.subscribe functional",
  runUnconditionalServerTest(async (harness) => {
    assert.strictEqual(harness.environment.descriptor.orchestrationProtocolVersion, 2);
    assert.strictEqual(typeof harness.environment.projectWorkflowBoard, "boolean");

    const rawConfig = await harness.rpc.callUnary("server.getConfig", {});
    const decoded = decodeServerConfig(rawConfig);
    assert.strictEqual(decoded.settings.workflowsEnabled, false);

    await harness.rpc.callUnary("server.updateSettings", {
      patch: { workflowsEnabled: true },
    });
    const updatedSettings = await harness.rpc.callUnary<{ readonly workflowsEnabled?: boolean }>(
      "server.getSettings",
      {},
    );
    assert.strictEqual(updatedSettings.workflowsEnabled, true);

    const wfSub = harness.rpc.subscribe("workflows.subscribe", {});
    const firstSnap = (await wfSub.nextChunk()) as { readonly workflows?: unknown };
    assert.isDefined(firstSnap);
    await wfSub.interrupt();
  }),
);

it.live(
  "[UNCOND-S-02] returns UnsupportedRpcMethod for workflowBoard.* when capability is absent without closing the WS connection",
  runUnconditionalServerTest(async (harness) => {
    if (harness.environment.projectWorkflowBoard) {
      return;
    }

    await expectRpcFailure(
      () => harness.rpc.callUnary("workflowBoard.getCard", { cardId: "card-1" }),
      "UnsupportedRpcMethod",
    );
    await expectRpcFailure(
      () =>
        harness.rpc.callUnary("workflowBoard.createCard", {
          projectId: "proj-1",
          title: "Legacy check",
        }),
      "UnsupportedRpcMethod",
    );

    const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId: "proj-1" });
    await expectRpcFailure(() => sub.nextChunk(), "UnsupportedRpcMethod");

    const probe = await harness.rpc.callUnary<{ readonly serverTime: string }>("server.probe", {});
    assert.isString(probe.serverTime);
  }),
);

it.live(
  "[UNCOND-S-03] enforces WS auth scopes for orchestration:read tokens on workflowBoard.createCard and workflowBoard.approveSpec",
  runUnconditionalServerTest(async (harness, binaryPath) => {
    const readClient = await pairScopedWsClient(harness, binaryPath, "e2e-read-only", [
      "orchestration:read",
    ]);
    try {
      const probe = await readClient.callUnary<{ readonly serverTime: string }>("server.probe", {});
      assert.isString(probe.serverTime);

      const createErr = await expectRpcFailure(() =>
        readClient.callUnary("workflowBoard.createCard", {
          projectId: "proj-1",
          title: "Unauthorized card",
        }),
      );
      const approveErr = await expectRpcFailure(() =>
        readClient.callUnary("workflowBoard.approveSpec", {
          cardId: "card-1",
          expectedRevision: 1,
        }),
      );

      if (
        harness.environment.projectWorkflowBoard ||
        createErr.includes("EnvironmentAuthorizationError")
      ) {
        assert.include(createErr, "EnvironmentAuthorizationError");
        assert.include(approveErr, "EnvironmentAuthorizationError");
      } else {
        assert.include(createErr, "UnsupportedRpcMethod");
        assert.include(approveErr, "UnsupportedRpcMethod");
      }
    } finally {
      readClient.close();
    }
  }),
);

it.live(
  "[F1-T1-05] workflowBoard.subscribe streams compact card summaries without heavy artifacts, diffs, or transcripts",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f1-t1-05",
      "F1 Subscribe Project",
      harness.workspaceDir,
    );
    const largeBody = "Acceptance detail ".repeat(500);
    await harness.rpc.callUnary("workflowBoard.createCard", {
      projectId,
      title: "Compact summary card",
      brief: largeBody,
      sourceType: "idea",
      assistanceMode: "balanced",
    });

    const sub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId });
    try {
      const snap = (await sub.nextChunk()) as {
        readonly cards?: ReadonlyArray<Record<string, unknown>>;
      };
      assert.isArray(snap.cards);
      assert.strictEqual(snap.cards?.length, 1);
      const card = snap.cards?.[0] ?? {};
      assert.isString(card.cardId);
      assert.strictEqual(card.title, "Compact summary card");
      assert.isString(card.stage);
      assert.isNumber(card.revision);
      assert.isUndefined(card.specMarkdown);
      assert.isUndefined(card.planMarkdown);
      assert.isUndefined(card.diff);
      assert.isUndefined(card.transcript);
      await sub.drainWithProbe();
    } finally {
      await sub.interrupt();
    }
  }),
);

it.live(
  "[F1-T2-03] workflowBoard.subscribe isolates projects and rejects unknown projectId without first-project fallback",
  runPlanningTest(async (harness) => {
    const projectA = await registerProject(harness, "proj-f1-a", "Project A", harness.workspaceDir);
    const workspaceB = initIsolatedGitRepo(
      NodePath.join(harness.tempRoot, "workspace-b"),
      "# Workspace B\n",
    );
    const projectB = await registerProject(harness, "proj-f1-b", "Project B", workspaceB.repoDir);
    const emptySub = harness.rpc.subscribe("workflowBoard.subscribe", { projectId: projectB });
    try {
      const emptySnapshot = (await emptySub.nextChunk()) as {
        readonly projectId: string;
        readonly cards: ReadonlyArray<unknown>;
      };
      assert.strictEqual(emptySnapshot.projectId, projectB);
      assert.deepStrictEqual(emptySnapshot.cards, []);
    } finally {
      await emptySub.interrupt();
    }

    await harness.rpc.callUnary("workflowBoard.createCard", {
      projectId: projectA,
      title: "Only in A",
      brief: "Scoped to A",
      sourceType: "idea",
      assistanceMode: "direct",
    });
    await harness.rpc.callUnary("workflowBoard.createCard", {
      projectId: projectB,
      title: "Only in B",
      brief: "Scoped to B",
      sourceType: "idea",
      assistanceMode: "direct",
    });

    const subA = harness.rpc.subscribe("workflowBoard.subscribe", { projectId: projectA });
    try {
      const snapA = (await subA.nextChunk()) as {
        readonly cards?: ReadonlyArray<{ readonly title: string }>;
      };
      assert.deepStrictEqual(
        snapA.cards?.map((c) => c.title),
        ["Only in A"],
      );
    } finally {
      await subA.interrupt();
    }

    const badSub = harness.rpc.subscribe("workflowBoard.subscribe", {
      projectId: "proj-does-not-exist",
    });
    try {
      await expectRpcFailure(() => badSub.nextChunk(), "project_not_found");
    } finally {
      await badSub.interrupt();
    }
  }),
);

it.live(
  "[F1-T2-04] enforces orchestration:read auth scope on workflowBoard.subscribe",
  runPlanningTest(async (harness, binaryPath) => {
    const noOrchClient = await pairScopedWsClient(harness, binaryPath, "e2e-diag-only", [
      "diagnostics:read",
    ]);
    try {
      const deniedSub = noOrchClient.subscribe("workflowBoard.subscribe", { projectId: "proj-1" });
      await expectRpcFailure(() => deniedSub.nextChunk(), "EnvironmentAuthorizationError");
    } finally {
      noOrchClient.close();
    }

    const readClient = await pairScopedWsClient(harness, binaryPath, "e2e-orch-read", [
      "orchestration:read",
    ]);
    try {
      const projectId = await registerProject(
        harness,
        "proj-f1-auth",
        "Auth Project",
        harness.workspaceDir,
      );
      const allowedSub = readClient.subscribe("workflowBoard.subscribe", { projectId });
      try {
        const snap = (await allowedSub.nextChunk()) as { readonly cards?: unknown };
        assert.isArray(snap.cards);
      } finally {
        await allowedSub.interrupt();
      }
    } finally {
      readClient.close();
    }
  }),
);

it.live(
  "[F2-T1-04] workflowBoard.getCard and getArtifact retrieve full detail and markdown on demand",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f2-t1-04",
      "F2 Detail Project",
      harness.workspaceDir,
    );
    const card = await createSpecAndApprove(harness, projectId, "F2-04");
    const detail = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly specApproval?: { readonly status: string; readonly digest?: string };
    }>("workflowBoard.getCard", { cardId: card.cardId });
    assert.strictEqual(detail.cardId, card.cardId);
    assert.strictEqual(detail.specApproval?.status, "valid");

    const artifact = await harness.rpc.callUnary<{ readonly content: string }>(
      "workflowBoard.getArtifact",
      { cardId: card.cardId, key: "spec.md" },
    );
    assert.include(artifact.content, "AC-01");
  }),
);

it.live(
  "[F2-T2-01] rejects missing cardId, cross-project cardId mismatch, and path-traversal artifact keys",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f2-t2-01",
      "F2 Traversal Project",
      harness.workspaceDir,
    );
    const card = await createSpecAndApprove(harness, projectId, "F2-T2-01");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.getCard", { cardId: "nonexistent-card-id" }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.getCard", {
        cardId: card.cardId,
        projectId: "other-project-id",
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.getArtifact", {
        cardId: card.cardId,
        key: "../../.env",
      }),
    );
  }),
);

it.live(
  "[F3-T1-04] workflowBoard.returnForRevision preserves history and marks downstream approvals stale",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f3-t1-04",
      "F3 Return Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F3-04");
    const planCard = await populateAndApprovePlan(harness, specCard);

    const returned = await harness.rpc.callUnary<{
      readonly stage: string;
      readonly revision: number;
      readonly planApproval?: { readonly status: string };
      readonly history?: ReadonlyArray<unknown>;
    }>("workflowBoard.returnForRevision", {
      cardId: planCard.cardId,
      expectedRevision: planCard.revision,
      targetStage: "spec",
      reason: "Clarify AC-02 edge cases",
    });

    assert.strictEqual(returned.stage, "spec");
    assert.isAbove(returned.revision, planCard.revision);
    assert.strictEqual(returned.planApproval?.status, "stale");
    assert.isAbove(returned.history?.length ?? 0, 0);
  }),
);

it.live(
  "[F3-T2-02] rejects concurrent returnForRevision with stale expectedRevision and leaves card unmutated",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f3-t2-02",
      "F3 Concurrency Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F3-T2-02");
    const planCard = await populateAndApprovePlan(harness, specCard);

    const firstReturn = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly stage: string;
    }>("workflowBoard.returnForRevision", {
      cardId: planCard.cardId,
      expectedRevision: planCard.revision,
      targetStage: "spec",
      reason: "First transition wins",
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.returnForRevision", {
        cardId: planCard.cardId,
        expectedRevision: planCard.revision,
        targetStage: "spec",
        reason: "Stale client transition",
      }),
    );

    const detail = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly stage: string;
    }>("workflowBoard.getCard", { cardId: planCard.cardId });
    assert.strictEqual(detail.revision, firstReturn.revision);
    assert.strictEqual(detail.stage, "spec");
    const serialized = JSON.stringify(detail);
    assert.include(serialized, "First transition wins");
    assert.notInclude(serialized, "Stale client transition");
  }),
);

it.live(
  "[F3-T2-03] rejects returnForRevision with whitespace reason or invalid targetStage",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f3-t2-03",
      "F3 Invalid Return Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F3-T2-03");
    const planCard = await populateAndApprovePlan(harness, specCard);

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.returnForRevision", {
        cardId: planCard.cardId,
        expectedRevision: planCard.revision,
        targetStage: "spec",
        reason: "   ",
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.returnForRevision", {
        cardId: planCard.cardId,
        expectedRevision: planCard.revision,
        targetStage: "ship",
        reason: "Cannot skip forward",
      }),
    );
  }),
);

it.live(
  "[F3-T2-04] rejects returnForRevision when authenticated as an agent/thread MCP token",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f3-t2-04",
      "F3 Agent Guard Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F3-T2-04");
    const planCard = await populateAndApprovePlan(harness, specCard);

    const agentClient = await mintAgentWsClient(harness, "thread:run-1", [
      "orchestration:read",
      "orchestration:operate",
    ]);
    try {
      const errMsg = await expectRpcFailure(
        () =>
          agentClient.callUnary("workflowBoard.returnForRevision", {
            cardId: planCard.cardId,
            expectedRevision: planCard.revision,
            targetStage: "spec",
            reason: "Agent should not override human return",
          }),
        "EnvironmentAuthorizationError",
      );
      assert.include(errMsg, "client:interactive");
    } finally {
      agentClient.close();
    }

    const detail = await harness.rpc.callUnary<{
      readonly stage: string;
      readonly revision: number;
    }>("workflowBoard.getCard", { cardId: planCard.cardId });
    assert.strictEqual(detail.stage, "execute");
    assert.strictEqual(detail.revision, planCard.revision);
  }),
);

it.live(
  "[F4-T1-04] treats identical brief resubmission idempotently and returns structured diff on modified brief",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f4-t1-04",
      "F4 Brief Project",
      harness.workspaceDir,
    );
    const first = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Idempotent brief",
      brief: "Initial specification text",
      sourceType: "idea",
      assistanceMode: "guided",
    });

    const repeat = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.updateSpec", {
      cardId: first.cardId,
      expectedRevision: first.revision,
      brief: "Initial specification text",
    });
    assert.strictEqual(repeat.cardId, first.cardId);
    assert.strictEqual(repeat.revision, first.revision);

    const modified = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly diff?: unknown;
    }>("workflowBoard.updateSpec", {
      cardId: first.cardId,
      expectedRevision: repeat.revision,
      brief: "Updated specification text with new constraint",
    });
    assert.isAbove(modified.revision, repeat.revision);
    assert.isDefined(modified.diff);
    assert.isNotEmpty(JSON.stringify(modified.diff));
  }),
);

it.live(
  "[F4-T2-01] rejects empty brief, binary/executable brief paths, and unknown assistanceMode",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f4-t2-01",
      "F4 Validation Project",
      harness.workspaceDir,
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.createCard", {
        projectId,
        title: "Empty brief",
        brief: "   ",
        sourceType: "idea",
        assistanceMode: "guided",
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.createCard", {
        projectId,
        title: "Bad file type",
        briefPath: "scripts/install.sh",
        sourceType: "file",
        assistanceMode: "balanced",
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.createCard", {
        projectId,
        title: "Bad mode",
        brief: "Valid brief",
        sourceType: "idea",
        assistanceMode: "auto",
      }),
    );
  }),
);

it.live(
  "[F4-T2-05] rejects createCard with missing or unknown projectId without falling back to active project",
  runPlanningTest(async (harness) => {
    await registerProject(harness, "proj-f4-active", "Active Project", harness.workspaceDir);

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.createCard", {
        title: "Missing project",
        brief: "Must fail explicitly",
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.createCard", {
        projectId: "proj-unknown-999",
        title: "Unknown project",
        brief: "Must fail explicitly",
      }),
    );
  }),
);

it.live(
  "[F5-T1-04] workflowBoard.exploreStep enforces <=3 cap and requires explicit continueRound opt-in",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f5-t1-04",
      "F5 Explore Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Guided exploration",
      brief: "Feature requiring 5 design decisions",
      sourceType: "idea",
      assistanceMode: "guided",
    });

    const round1 = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly pendingRoundPrompt?: boolean;
      readonly roundIndex?: number;
    }>("workflowBoard.exploreStep", {
      cardId: created.cardId,
      expectedRevision: created.revision,
      answers: [
        { decisionId: "DEC-1", action: "edit", answer: "Use SQLite WAL mode" },
        { decisionId: "DEC-2", action: "edit", answer: "Keep single writer queue" },
        { decisionId: "DEC-3", action: "edit", answer: "Validate JSON schema" },
      ],
    });
    assert.isAbove(round1.revision, created.revision);
    assert.strictEqual(round1.roundIndex, 1);
    if (round1.pendingRoundPrompt !== undefined) {
      assert.isTrue(round1.pendingRoundPrompt);
    }

    const beforeOptIn = await harness.rpc.callUnary<{
      readonly exploration?: { readonly roundIndex?: number };
    }>("workflowBoard.getCard", { cardId: created.cardId });
    assert.notStrictEqual(beforeOptIn.exploration?.roundIndex, 2);

    const round2 = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly roundIndex?: number;
    }>("workflowBoard.exploreStep", {
      cardId: created.cardId,
      expectedRevision: round1.revision,
      continueRound: true,
      answers: [
        { decisionId: "DEC-4", action: "edit", answer: "Bind approvals to digest" },
        { decisionId: "DEC-5", action: "edit", answer: "Journal artifact writes" },
      ],
    });
    assert.isAbove(round2.revision, round1.revision);
    assert.strictEqual(round2.roundIndex, 2);
  }),
);

it.live(
  "[F5-T2-01] blocks approveSpec when exploration decisions remain unresolved or marked unsure",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f5-t2-01",
      "F5 Unsure Block Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Unsure decision",
      brief: "Feature with unsure decision",
      sourceType: "idea",
      assistanceMode: "guided",
    });

    const specified = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updateSpec",
      {
        cardId: created.cardId,
        expectedRevision: created.revision,
        acceptanceCriteria: [{ id: "AC-1", text: "Feature can be verified." }],
      },
    );

    const explored = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.exploreStep",
      {
        cardId: created.cardId,
        expectedRevision: specified.revision,
        answers: [{ decisionId: "DEC-1", action: "unsure" }],
      },
    );

    await expectRpcFailure(
      () =>
        harness.rpc.callUnary("workflowBoard.approveSpec", {
          cardId: created.cardId,
          expectedRevision: explored.revision,
        }),
      "exploration_unresolved",
    );
  }),
);

it.live(
  "[F5-T2-02] rejects >3 decisions in exploreStep batch and keeps pause/explain/unsure unresolved",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f5-t2-02",
      "F5 Cap Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Cap enforcement",
      brief: "Test exploration round size cap",
      sourceType: "idea",
      assistanceMode: "guided",
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.exploreStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        answers: [
          { decisionId: "D1", action: "edit", answer: "A" },
          { decisionId: "D2", action: "edit", answer: "B" },
          { decisionId: "D3", action: "edit", answer: "C" },
          { decisionId: "D4", action: "edit", answer: "D" },
        ],
      }),
    );

    const paused = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly exploration?: { readonly unresolvedIds?: ReadonlyArray<string> };
    }>("workflowBoard.exploreStep", {
      cardId: created.cardId,
      expectedRevision: created.revision,
      answers: [{ decisionId: "D1", action: "pause" }],
    });
    assert.isAbove(paused.revision, created.revision);
    assert.include(paused.exploration?.unresolvedIds ?? [], "D1");

    const afterExplainUnsure = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly exploration?: { readonly unresolvedIds?: ReadonlyArray<string> };
    }>("workflowBoard.exploreStep", {
      cardId: created.cardId,
      expectedRevision: paused.revision,
      answers: [
        { decisionId: "D2", action: "explain" },
        { decisionId: "D3", action: "unsure" },
      ],
    });
    assert.isAbove(afterExplainUnsure.revision, paused.revision);
    assert.includeMembers(
      [...(afterExplainUnsure.exploration?.unresolvedIds ?? [])],
      ["D1", "D2", "D3"],
    );
  }),
);

it.live(
  "[F5-T2-04] isolates exploration decisions from execution DAG tasks and challenge concern cap",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f5-t2-04",
      "F5 Isolation Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Isolation check",
      brief: "Decisions should not inject DAG tasks",
      sourceType: "idea",
      assistanceMode: "guided",
    });

    await harness.rpc.callUnary("workflowBoard.exploreStep", {
      cardId: created.cardId,
      expectedRevision: created.revision,
      answers: [{ decisionId: "DEC-ISO-1", action: "edit", answer: "Resolved choice" }],
    });

    const detail = await harness.rpc.callUnary<{
      readonly tasks?: ReadonlyArray<unknown>;
      readonly maxChallengeConcerns?: number;
    }>("workflowBoard.getCard", { cardId: created.cardId });
    assert.strictEqual(detail.tasks?.length ?? 0, 0);
    if (detail.maxChallengeConcerns !== undefined) {
      assert.isAtMost(detail.maxChallengeConcerns, 3);
    }
  }),
);

it.live(
  "[F5-T2-05] rejects concurrent exploreStep with stale expectedRevision without overwriting first answer",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f5-t2-05",
      "F5 Concurrency Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Explore race",
      brief: "Optimistic concurrency test",
      sourceType: "idea",
      assistanceMode: "guided",
    });

    const won = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.exploreStep",
      {
        cardId: created.cardId,
        expectedRevision: created.revision,
        answers: [{ decisionId: "DEC-1", action: "edit", answer: "Winner answer" }],
      },
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.exploreStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        answers: [{ decisionId: "DEC-1", action: "edit", answer: "Loser answer" }],
      }),
    );

    const detail = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.getCard",
      { cardId: created.cardId },
    );
    assert.strictEqual(detail.revision, won.revision);
    const serialized = JSON.stringify(detail);
    assert.include(serialized, "Winner answer");
    assert.notInclude(serialized, "Loser answer");
  }),
);

it.live(
  "[F6-T1-04] getCard and getArtifact return wireframe HTML when enabled and wireframesIndex: null for non-UI cards",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f6-t1-04",
      "F6 Wireframe Project",
      harness.workspaceDir,
    );
    const uiCard = await createSpecAndApprove(harness, projectId, "UI", { withWireframes: true });
    const nonUiCard = await createSpecAndApprove(harness, projectId, "API", {
      withWireframes: false,
    });

    const uiDetail = await harness.rpc.callUnary<{ readonly wireframesIndex: string | null }>(
      "workflowBoard.getCard",
      { cardId: uiCard.cardId },
    );
    assert.isString(uiDetail.wireframesIndex);

    const wfArtifact = await harness.rpc.callUnary<{ readonly content: string }>(
      "workflowBoard.getArtifact",
      { cardId: uiCard.cardId, key: uiDetail.wireframesIndex ?? "wireframes/index.html" },
    );
    assert.isString(wfArtifact.content);
    assert.isNotEmpty(wfArtifact.content);

    const nonUiDetail = await harness.rpc.callUnary<{ readonly wireframesIndex: string | null }>(
      "workflowBoard.getCard",
      { cardId: nonUiCard.cardId },
    );
    assert.isNull(nonUiDetail.wireframesIndex);
  }),
);

it.live(
  "[F6-T2-03] rejects getArtifact wireframe fetch on non-UI card or path-traversal wireframe key",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f6-t2-03",
      "F6 Traversal Project",
      harness.workspaceDir,
    );
    const nonUiCard = await createSpecAndApprove(harness, projectId, "NonUI", {
      withWireframes: false,
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.getArtifact", {
        cardId: nonUiCard.cardId,
        key: "wireframes/index.html",
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.getArtifact", {
        cardId: nonUiCard.cardId,
        key: "wireframes/../../secret.html",
      }),
    );
  }),
);

it.live(
  "[F7-T1-05] workflowBoard.challengeStep binds receipt to spec/interview digests and updates binding on synthesis",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f7-t1-05",
      "F7 Challenge Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Challenge receipt test",
      brief: "Verify challenge receipt binding",
      sourceType: "idea",
      assistanceMode: "balanced",
    });

    const specUpdated = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly specDigest?: string;
    }>("workflowBoard.updateSpec", {
      cardId: created.cardId,
      expectedRevision: created.revision,
      acceptanceCriteria: [{ id: "AC-01", text: "Preserve pre-edit backup revision." }],
      constraints: ["No auto-commits"],
      sourceRefs: ["README.md"],
    });

    const challenged = await harness.rpc.callUnary<{
      readonly revision: number;
      readonly challengeReceipt?: {
        readonly synthesisCount?: number;
        readonly specDigest?: string;
        readonly interviewDigest?: string;
      };
    }>("workflowBoard.challengeStep", {
      cardId: created.cardId,
      expectedRevision: specUpdated.revision,
      action: "synthesize",
      concerns: [
        {
          id: "CON-1",
          summary: "Rollback path missing",
          disposition: "answer",
          answer: "Keep exact pre-edit backup revision.",
        },
      ],
    });
    assert.isAbove(challenged.revision, specUpdated.revision);
    assert.isDefined(challenged.challengeReceipt);
    assert.strictEqual(challenged.challengeReceipt?.synthesisCount, 1);
    assert.isString(challenged.challengeReceipt?.specDigest);
    assert.isString(challenged.challengeReceipt?.interviewDigest);

    const detail = await harness.rpc.callUnary<{
      readonly challengeReceipt?: {
        readonly synthesisCount?: number;
        readonly specDigest?: string;
        readonly interviewDigest?: string;
      };
    }>("workflowBoard.getCard", { cardId: created.cardId });
    assert.strictEqual(detail.challengeReceipt?.synthesisCount, 1);
    assert.strictEqual(
      detail.challengeReceipt?.specDigest,
      challenged.challengeReceipt?.specDigest,
    );
    assert.strictEqual(
      detail.challengeReceipt?.interviewDigest,
      challenged.challengeReceipt?.interviewDigest,
    );
  }),
);

it.live(
  "[F7-T2-01] rejects >3 challenge concerns and rejects dismissal without a non-empty reason",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f7-t2-01",
      "F7 Validation Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Challenge bounds",
      brief: "Test max 3 concerns and dismiss reason",
      sourceType: "idea",
      assistanceMode: "balanced",
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "resolve",
        concerns: [
          { id: "C1", summary: "1", disposition: "answer", answer: "a" },
          { id: "C2", summary: "2", disposition: "answer", answer: "b" },
          { id: "C3", summary: "3", disposition: "answer", answer: "c" },
          { id: "C4", summary: "4", disposition: "answer", answer: "d" },
        ],
      }),
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "resolve",
        concerns: [{ id: "C1", summary: "1", disposition: "dismiss", reason: "   " }],
      }),
    );
  }),
);

it.live(
  "[F7-T2-02] rejects 2nd synthesis pass and rejects synthesis that mutates verbatim human answers or sets approved",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f7-t2-02",
      "F7 Synthesis Guard Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Synthesis limit",
      brief: "Single synthesis pass guard",
      sourceType: "idea",
      assistanceMode: "balanced",
    });

    const firstSynth = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.challengeStep",
      {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "synthesize",
        concerns: [
          {
            id: "C1",
            summary: "Scope check",
            disposition: "answer",
            answer: "Verbatim human constraint: preserve SQLite FK.",
          },
        ],
      },
    );

    const detailAfterSynth = await harness.rpc.callUnary<{
      readonly specApproval?: { readonly status: string };
    }>("workflowBoard.getCard", { cardId: created.cardId });
    assert.include(
      JSON.stringify(detailAfterSynth),
      "Verbatim human constraint: preserve SQLite FK.",
    );
    assert.notStrictEqual(detailAfterSynth.specApproval?.status, "valid");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: created.cardId,
        expectedRevision: firstSynth.revision,
        action: "synthesize",
        concerns: [],
      }),
    );

    const guardedCard = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Synthesis mutation guard",
      brief: "Reject synthesis mutating human answer or setting approved",
      sourceType: "idea",
      assistanceMode: "balanced",
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: guardedCard.cardId,
        expectedRevision: guardedCard.revision,
        action: "synthesize",
        approved: true,
        concerns: [
          {
            id: "C1",
            summary: "Auto-approve bypass attempt",
            disposition: "answer",
            answer: "Original human answer",
            mutatedAnswer: "Rewritten answer",
          },
        ],
      }),
    );
  }),
);

it.live(
  "[F7-T2-03] rejects wrong manual fallback token and rejects fallback bypass when unresolved automated concern exists",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f7-t2-03",
      "F7 Fallback Guard Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Manual fallback guard",
      brief: "Exact I acknowledge check",
      sourceType: "idea",
      assistanceMode: "balanced",
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "manualFallback",
        acknowledgementToken: "i acknowledge",
        answers: ["q1", "q2", "q3"],
      }),
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "manualFallback",
        acknowledgementToken: "I acknowledge",
        unresolvedConcernIds: ["C1"],
        answers: ["q1", "q2", "q3"],
      }),
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.challengeStep", {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "manualFallback",
        acknowledgementToken: "I acknowledge",
        unresolvedConcernIds: [],
        answers: ["q1", "q2"],
      }),
    );

    const acceptedFallback = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.challengeStep",
      {
        cardId: created.cardId,
        expectedRevision: created.revision,
        action: "manualFallback",
        acknowledgementToken: "I acknowledge",
        unresolvedConcernIds: [],
        answers: ["q1", "q2", "q3"],
      },
    );
    assert.isAbove(acceptedFallback.revision, created.revision);
  }),
);

it.live(
  "[F8-T1-02] workflowBoard.approveSpec issues content-bound receipt matching exact specDigest",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f8-t1-02",
      "F8 Approve Project",
      harness.workspaceDir,
    );
    const approved = await createSpecAndApprove(harness, projectId, "F8-02");
    const detail = await harness.rpc.callUnary<{
      readonly specApproval?: { readonly status: string; readonly specDigest: string };
    }>("workflowBoard.getCard", { cardId: approved.cardId });
    assert.strictEqual(detail.specApproval?.status, "valid");
    assert.isString(detail.specApproval?.specDigest);
  }),
);

it.live(
  "[F8-T1-03] editing an approved Spec invalidates both Spec and downstream Plan approvals",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f8-t1-03",
      "F8 Cascade Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F8-03");
    const planCard = await populateAndApprovePlan(harness, specCard);

    const edited = await harness.rpc.callUnary<{
      readonly specApproval?: { readonly status: string };
      readonly planApproval?: { readonly status: string };
    }>("workflowBoard.updateSpec", {
      cardId: planCard.cardId,
      expectedRevision: planCard.revision,
      brief: "Revised spec requirement that invalidates downstream plan.",
    });

    assert.strictEqual(edited.specApproval?.status, "stale");
    assert.strictEqual(edited.planApproval?.status, "stale");
  }),
);

it.live(
  "[F8-T2-01] rejects workflowBoard.approveSpec from an agent/thread MCP token",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f8-t2-01",
      "F8 Agent Reject Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Agent cannot approve",
      brief: "Human-only gate",
      sourceType: "idea",
      assistanceMode: "direct",
    });
    const updatedSpec = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updateSpec",
      {
        cardId: created.cardId,
        expectedRevision: created.revision,
        acceptanceCriteria: [{ id: "AC-01", text: "Human approval required." }],
        constraints: ["No agent approval"],
        sourceRefs: ["README.md"],
        wireframesIndex: null,
      },
    );
    const challenged = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.challengeStep",
      {
        cardId: created.cardId,
        expectedRevision: updatedSpec.revision,
        action: "complete",
        concerns: [],
      },
    );

    const agentClient = await mintAgentWsClient(harness, "thread:agent-1", [
      "orchestration:read",
      "orchestration:operate",
    ]);
    try {
      const errMsg = await expectRpcFailure(
        () =>
          agentClient.callUnary("workflowBoard.approveSpec", {
            cardId: created.cardId,
            expectedRevision: challenged.revision,
          }),
        "EnvironmentAuthorizationError",
      );
      assert.include(errMsg, "client:interactive");
    } finally {
      agentClient.close();
    }

    const detail = await harness.rpc.callUnary<{
      readonly specApproval?: { readonly status: string };
    }>("workflowBoard.getCard", { cardId: created.cardId });
    assert.notStrictEqual(detail.specApproval?.status, "valid");
  }),
);

it.live(
  "[F8-T2-02] rejects workflowBoard.approveSpec when acceptance criteria are empty or open decisions exist",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f8-t2-02",
      "F8 Missing Prereq Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Missing AC",
      brief: "No acceptance criteria provided",
      sourceType: "idea",
      assistanceMode: "direct",
    });

    await expectRpcFailure(
      () =>
        harness.rpc.callUnary("workflowBoard.approveSpec", {
          cardId: created.cardId,
          expectedRevision: created.revision,
        }),
      "spec approval requires at least one acceptance criterion",
    );
  }),
);

it.live(
  "[F8-T2-03] rejects workflowBoard.approveSpec with stale expectedRevision or mismatched expectedDigest",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f8-t2-03",
      "F8 Revision Conflict Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Stale spec approve",
      brief: "Initial brief",
      sourceType: "idea",
      assistanceMode: "direct",
    });

    const updated = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updateSpec",
      {
        cardId: created.cardId,
        expectedRevision: created.revision,
        acceptanceCriteria: [{ id: "AC-01", text: "Works." }],
      },
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approveSpec", {
        cardId: created.cardId,
        expectedRevision: created.revision,
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approveSpec", {
        cardId: created.cardId,
        expectedRevision: updated.revision,
        expectedDigest: "sha256:0000000000000000",
      }),
    );
  }),
);

it.live(
  "[F9-T1-03] workflowBoard.updatePlan persists 6 design lenses while preserving Spec approval validity",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f9-t1-03",
      "F9 Lenses Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F9-03");

    const updated = await harness.rpc.callUnary<{
      readonly specApproval?: { readonly status: string };
      readonly lenses?: ReadonlyArray<{ readonly lens: string }>;
    }>("workflowBoard.updatePlan", {
      cardId: specCard.cardId,
      expectedRevision: specCard.revision,
      lenses: validSixLenses(false),
    });

    assert.strictEqual(updated.specApproval?.status, "valid");
    assert.strictEqual(updated.lenses?.length, 6);
  }),
);

it.live(
  "[F9-T2-01] rejects workflowBoard.approvePlan when any of the 6 lenses is omitted or undecided",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f9-t2-01",
      "F9 Missing Lens Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F9-T2-01");

    const incomplete = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: validSixLenses(true).filter((entry) => entry.lens !== "delivery"),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Selected A",
        strategy: "outcomes",
        granularity: "balanced",
        tasks: validDisjointTasks(),
        acMappings: validAcMappings(),
      },
    );

    await expectRpcFailure(
      () =>
        harness.rpc.callUnary("workflowBoard.approvePlan", {
          cardId: specCard.cardId,
          expectedRevision: incomplete.revision,
        }),
      "delivery",
    );
  }),
);

it.live(
  "[F9-T2-02] rejects not-applicable lens without reason and applicable lens with empty design summary",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f9-t2-02",
      "F9 Empty Lens Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F9-T2-02");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: [{ lens: "ux", applicable: false, reason: "" }, ...validSixLenses(true).slice(1)],
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: [
          ...validSixLenses(true).slice(0, 2),
          { lens: "architecture", applicable: true, summary: "   " },
          ...validSixLenses(true).slice(3),
        ],
      }),
    );
  }),
);

it.live(
  "[F9-T2-04] rejects unknown design lens literal in workflowBoard.updatePlan",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f9-t2-04",
      "F9 Bad Lens Key Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F9-T2-04");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: [{ lens: "marketing", applicable: true, summary: "Invalid lens" }],
      }),
    );
  }),
);

it.live(
  "[F9-T2-05] rejects workflowBoard.approvePlan when upstream Spec approval is missing or stale",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f9-t2-05",
      "F9 Unapproved Spec Project",
      harness.workspaceDir,
    );
    const created = await harness.rpc.callUnary<{
      readonly cardId: string;
      readonly revision: number;
    }>("workflowBoard.createCard", {
      projectId,
      title: "Unapproved spec plan",
      brief: "Cannot approve plan before spec",
      sourceType: "idea",
      assistanceMode: "direct",
    });

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: created.cardId,
        expectedRevision: created.revision,
      }),
    );
  }),
);

it.live(
  "[F10-T1-03] persists 2 design alternatives, selection, and verbatim rationale across getCard",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f10-t1-03",
      "F10 Alternatives Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F10-03");
    const rationale = "Alt A preserves strict SQLite transactional boundaries.";

    await harness.rpc.callUnary("workflowBoard.updatePlan", {
      cardId: specCard.cardId,
      expectedRevision: specCard.revision,
      lenses: validSixLenses(true),
      alternatives: validTwoAlternatives(),
      selectedAlternativeId: "alt-a",
      selectionRationale: rationale,
    });

    const detail = await harness.rpc.callUnary<{
      readonly alternatives?: ReadonlyArray<unknown>;
      readonly selectedAlternativeId?: string;
      readonly selectionRationale?: string;
    }>("workflowBoard.getCard", { cardId: specCard.cardId });
    assert.strictEqual(detail.alternatives?.length, 2);
    assert.strictEqual(detail.selectedAlternativeId, "alt-a");
    assert.strictEqual(detail.selectionRationale, rationale);
  }),
);

it.live(
  "[F10-T2-01] rejects 0, 1, 3+ design alternatives or duplicate identical alternatives",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f10-t2-01",
      "F10 Count Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F10-T2-01");
    const [altA, altB] = validTwoAlternatives();

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        alternatives: [altA],
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        alternatives: [
          altA,
          altB,
          { id: "alt-c", title: "Third", summary: "Extra", tradeoffs: "Too many" },
        ],
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        alternatives: [altA, { ...altA!, id: "alt-a-dup" }],
      }),
    );
  }),
);

it.live(
  "[F10-T2-02] rejects alternative selection without rationale and blocks approvePlan when unselected",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f10-t2-02",
      "F10 Rationale Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F10-T2-02");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: "   ",
      }),
    );

    const unselected = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: validSixLenses(true),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: null,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: validDisjointTasks(),
        acMappings: validAcMappings(),
      },
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: specCard.cardId,
        expectedRevision: unselected.revision,
      }),
    );
  }),
);

it.live(
  "[F10-T2-03] rejects 2nd comparison revision request after max 1 revision has been used",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f10-t2-03",
      "F10 Revision Limit Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F10-T2-03");

    const firstRev = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        alternatives: validTwoAlternatives(),
        reviseAlternativesFeedback: "Clarify migration cost in option B",
      },
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: firstRev.revision,
        alternatives: validTwoAlternatives(),
        reviseAlternativesFeedback: "Second revision must be rejected",
      }),
    );
  }),
);

it.live(
  "[F10-T2-05] rejects selecting an unknown alternativeId not in saved comparison pair",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f10-t2-05",
      "F10 Unknown Alt Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F10-T2-05");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-999",
        selectionRationale: "Nonexistent option",
      }),
    );
  }),
);

it.live(
  "[F11-T1-02] persists risk-first and expand-contract strategy metadata and validated dependencies",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f11-t1-02",
      "F11 Strategy Metadata Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F11-02");

    const riskFirst = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: validSixLenses(true),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Risk probe first",
        strategy: "risk-first",
        strategyMetadata: {
          uncertainty: "SQLite lock behavior on Windows",
          probeTaskId: "TSK-0001",
        },
        granularity: "balanced",
        tasks: validDisjointTasks(),
      },
    );

    const expandContract = await harness.rpc.callUnary<{
      readonly strategy?: string;
      readonly strategyMetadata?: Record<string, unknown>;
    }>("workflowBoard.updatePlan", {
      cardId: specCard.cardId,
      expectedRevision: riskFirst.revision,
      strategy: "expand-contract",
      strategyMetadata: {
        compatibilityWindow: "M2-M3",
        transitionStep: "Dual-write schema",
        retirementPrereq: "All clients migrated",
      },
    });
    assert.strictEqual(expandContract.strategy, "expand-contract");
    assert.isDefined(expandContract.strategyMetadata);
  }),
);

it.live(
  "[F11-T1-05] validates mutually exclusive filesAllowed across coarse, balanced, and fine granularities",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f11-t1-05",
      "F11 Granularity Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F11-05");

    let rev = specCard.revision;
    for (const granularity of ["coarse", "balanced", "fine"] as const) {
      const updated = await harness.rpc.callUnary<{
        readonly revision: number;
        readonly granularity?: string;
      }>("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: rev,
        lenses: validSixLenses(true),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: `Valid ${granularity} breakdown`,
        strategy: "outcomes",
        granularity,
        tasks: validDisjointTasks(),
      });
      assert.strictEqual(updated.granularity, granularity);
      rev = updated.revision;
    }
  }),
);

it.live(
  "[F11-T2-01] rejects overlapping filesAllowed across sequential or parallel tasks",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f11-t2-01",
      "F11 Overlap Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F11-T2-01");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-0001",
            title: "Task 1",
            dependsOn: [],
            filesAllowed: ["src/shared.ts"],
            filesForbidden: [],
            testingMethod: "test-first",
          },
          {
            taskId: "TSK-0002",
            title: "Task 2",
            dependsOn: ["TSK-0001"],
            filesAllowed: ["src/shared.ts"],
            filesForbidden: [],
            testingMethod: "existing-checks",
          },
        ],
      }),
    );
  }),
);

it.live(
  "[F11-T2-02] rejects cyclic task DAG dependencies and unknown dependency task IDs",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f11-t2-02",
      "F11 Cycle Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F11-T2-02");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-1",
            title: "A",
            dependsOn: ["TSK-2"],
            filesAllowed: ["src/a.ts"],
            filesForbidden: [],
            testingMethod: "test-first",
          },
          {
            taskId: "TSK-2",
            title: "B",
            dependsOn: ["TSK-1"],
            filesAllowed: ["src/b.ts"],
            filesForbidden: [],
            testingMethod: "test-first",
          },
        ],
      }),
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-1",
            title: "Missing dep",
            dependsOn: ["TSK-999"],
            filesAllowed: ["src/a.ts"],
            filesForbidden: [],
            testingMethod: "test-first",
          },
        ],
      }),
    );
  }),
);

it.live(
  "[F11-T2-03] rejects custom strategy without rationale, risk-first without probe task, and expand-contract without retirement prereq",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f11-t2-03",
      "F11 Strategy Guard Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F11-T2-03");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "custom",
        strategyRationale: "   ",
        granularity: "balanced",
        tasks: validDisjointTasks(),
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "risk-first",
        strategyMetadata: { uncertainty: "Unknown IO perf" },
        granularity: "balanced",
        tasks: validDisjointTasks(),
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "expand-contract",
        strategyMetadata: { compatibilityWindow: "M2" },
        granularity: "balanced",
        tasks: validDisjointTasks(),
      }),
    );
  }),
);

it.live(
  "[F11-T2-04] rejects empty filesAllowed, filesAllowed intersecting filesForbidden, and path traversal ../",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f11-t2-04",
      "F11 Path Contract Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F11-T2-04");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-1",
            title: "Empty allowed",
            dependsOn: [],
            filesAllowed: [],
            filesForbidden: [],
            testingMethod: "test-first",
          },
        ],
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-1",
            title: "Conflict allowed/forbidden",
            dependsOn: [],
            filesAllowed: ["src/a.ts"],
            filesForbidden: ["src/a.ts"],
            testingMethod: "test-first",
          },
        ],
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        strategy: "outcomes",
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-1",
            title: "Path escape",
            dependsOn: [],
            filesAllowed: ["../outside.ts"],
            filesForbidden: [],
            testingMethod: "test-first",
          },
        ],
      }),
    );
  }),
);

it.live(
  "[F12-T1-02] workflowBoard.approvePlan binds spec, plan, tasks, checks, and AC mappings into planApproval receipt",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f12-t1-02",
      "F12 Approve Plan Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F12-02");
    const approved = await populateAndApprovePlan(harness, specCard);

    const detail = await harness.rpc.callUnary<{
      readonly planApproval?: {
        readonly status: string;
        readonly specDigest?: string;
        readonly planDigest?: string;
        readonly tasksDigest?: string;
      };
    }>("workflowBoard.getCard", { cardId: approved.cardId });
    assert.strictEqual(detail.planApproval?.status, "valid");
    assert.isString(detail.planApproval?.specDigest);
    assert.isString(detail.planApproval?.planDigest);
    assert.isString(detail.planApproval?.tasksDigest);
  }),
);

it.live(
  "[F12-T1-03] plan-only edit invalidates Plan approval while preserving Spec approval validity",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f12-t1-03",
      "F12 Plan-Only Edit Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F12-03");
    const planCard = await populateAndApprovePlan(harness, specCard);

    const editedPlan = await harness.rpc.callUnary<{
      readonly specApproval?: { readonly status: string };
      readonly planApproval?: { readonly status: string };
    }>("workflowBoard.updatePlan", {
      cardId: planCard.cardId,
      expectedRevision: planCard.revision,
      acMappings: [
        {
          criterionId: "AC-01",
          method: "automated",
          command: ["node", "--test", "src/validator-v2.test.ts"],
        },
        {
          criterionId: "AC-02",
          method: "manual",
          procedure: "Updated manual check procedure.",
        },
      ],
    });

    assert.strictEqual(editedPlan.planApproval?.status, "stale");
    assert.strictEqual(editedPlan.specApproval?.status, "valid");
  }),
);

it.live(
  "[F12-T2-01] rejects approvePlan when any AC-* is unmapped or unknown AC-999 is mapped",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f12-t2-01",
      "F12 Unmapped AC Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F12-T2-01");

    const partialMap = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: validSixLenses(true),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Partial AC mapping",
        strategy: "outcomes",
        granularity: "balanced",
        tasks: validDisjointTasks(),
        acMappings: [validAcMappings()[0]],
      },
    );

    await expectRpcFailure(
      () =>
        harness.rpc.callUnary("workflowBoard.approvePlan", {
          cardId: specCard.cardId,
          expectedRevision: partialMap.revision,
        }),
      "AC-02",
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: partialMap.revision,
        acMappings: [
          ...validAcMappings(),
          { criterionId: "AC-999", method: "manual", procedure: "Unknown criterion" },
        ],
      }),
    );
  }),
);

it.live(
  "[F12-T2-02] rejects shell-string check injection and vague manual checks without concrete procedure",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f12-t2-02",
      "F12 Argv Guard Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F12-T2-02");

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        acMappings: [
          { criterionId: "AC-01", method: "automated", command: ["sh", "-c", "a && b"] },
          validAcMappings()[1],
        ],
      }),
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.updatePlan", {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        acMappings: [
          validAcMappings()[0],
          { criterionId: "AC-02", method: "manual", procedure: "   " },
        ],
      }),
    );
  }),
);

it.live(
  "[F12-T2-03] rejects workflowBoard.approvePlan from agent/thread MCP token and when Spec approval is stale",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f12-t2-03",
      "F12 Auth & Stale Spec Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F12-T2-03");
    const readyPlan = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: validSixLenses(true),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: "Ready for approval",
        strategy: "outcomes",
        granularity: "balanced",
        tasks: validDisjointTasks(),
        acMappings: validAcMappings(),
      },
    );

    const agentClient = await mintAgentWsClient(harness, "thread:agent-2", [
      "orchestration:read",
      "orchestration:operate",
    ]);
    try {
      const errMsg = await expectRpcFailure(
        () =>
          agentClient.callUnary("workflowBoard.approvePlan", {
            cardId: specCard.cardId,
            expectedRevision: readyPlan.revision,
          }),
        "EnvironmentAuthorizationError",
      );
      assert.include(errMsg, "client:interactive");
    } finally {
      agentClient.close();
    }

    const afterAgentAttempt = await harness.rpc.callUnary<{
      readonly planApproval?: { readonly status: string };
    }>("workflowBoard.getCard", { cardId: specCard.cardId });
    assert.notStrictEqual(afterAgentAttempt.planApproval?.status, "valid");

    const staledSpec = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updateSpec",
      {
        cardId: specCard.cardId,
        expectedRevision: readyPlan.revision,
        brief: "Spec edited before plan approval",
      },
    );
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: specCard.cardId,
        expectedRevision: staledSpec.revision,
      }),
    );
  }),
);

it.live(
  "[F12-T2-04] rejects approvePlan on stale expectedRevision or unreconciled external disk edit to plan.md",
  runPlanningTest(async (harness) => {
    const projectId = await registerProject(
      harness,
      "proj-f12-t2-04",
      "F12 External Edit Project",
      harness.workspaceDir,
    );
    const specCard = await createSpecAndApprove(harness, projectId, "F12-T2-04");
    const readyPlan = await harness.rpc.callUnary<{ readonly revision: number }>(
      "workflowBoard.updatePlan",
      {
        cardId: specCard.cardId,
        expectedRevision: specCard.revision,
        lenses: validSixLenses(true),
        alternatives: validTwoAlternatives(),
        selectedAlternativeId: "alt-a",
        selectionRationale: "External edit check",
        strategy: "outcomes",
        granularity: "balanced",
        tasks: validDisjointTasks(),
        acMappings: validAcMappings(),
      },
    );

    const planFile = NodePath.join(
      harness.workspaceDir,
      ".legion",
      "workflows",
      specCard.cardId,
      "plan.md",
    );
    assert.isTrue(NodeFS.existsSync(planFile), `Expected plan deliverable at ${planFile}`);
    NodeFS.appendFileSync(planFile, "\n<!-- external manual edit -->\n", "utf8");
    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: specCard.cardId,
        expectedRevision: readyPlan.revision,
      }),
    );

    await expectRpcFailure(() =>
      harness.rpc.callUnary("workflowBoard.approvePlan", {
        cardId: specCard.cardId,
        expectedRevision: Math.max(0, readyPlan.revision - 1),
      }),
    );
  }),
);

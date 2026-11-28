// @effect-diagnostics nodeBuiltinImport:off -- Parity suite seeds temporary workspaces and subprocess mocks.
import * as NodeChildProcess from "node:child_process";
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

import { assert } from "@effect/vitest";
import {
  IsoDateTime,
  MilestoneId,
  milestoneMatches,
  OrchestrationV2RpcSchemas,
  PEER_LINK_THREAD_HISTORY_ROUTE,
  PEER_LINK_THREAD_ITEM_ROUTE,
  PEER_LINK_THREAD_STREAM_ROUTE,
  ProjectId,
  ProviderInstanceId,
  RunId,
  ThreadId,
  TrimmedNonEmptyString,
  WsRpcGroup,
} from "@legioncode/contracts";
import { pastedUrl } from "@legioncode/shared/oauthCallback";
import { hasPendingBackgroundSubagent } from "@legioncode/shared/orchestrationV2PendingBackgroundWork";
import * as Schema from "effect/Schema";
import { fetch } from "undici";

import {
  buildWsRpcFixtures,
  writeMockProviderScripts,
  type ParityFixtureContext,
  type RpcFixtureSpec,
} from "./rustWireParityFixtures.ts";

interface ParityRpcFrame {
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

interface ParityRpcHarness {
  sendRequest(id: string, tag: string, payload: unknown): void;
  sendAck(requestId: string): void;
  sendInterrupt(requestId: string): void;
  nextFrameMatching(predicate: (frame: ParityRpcFrame) => boolean): Promise<ParityRpcFrame>;
  callUnary(id: string, tag: string, payload: unknown): Promise<unknown>;
  nextChunk(requestId: string, predicate?: (chunk: unknown) => boolean): Promise<unknown>;
}

interface WsRpcEntry {
  readonly payloadSchema: Schema.Codec<unknown>;
  readonly successSchema: Schema.Codec<unknown> & {
    readonly success?: Schema.Codec<unknown>;
    readonly error?: Schema.Codec<unknown>;
  };
  readonly errorSchema: Schema.Codec<unknown>;
}

interface CompiledRpcCodecs {
  readonly decodePayload: (input: unknown) => unknown;
  readonly decodeSuccess: (input: unknown) => unknown;
  readonly decodeError: (input: unknown) => unknown;
}

const decodeThreadProjection = Schema.decodeUnknownSync(
  Schema.toCodecJson(OrchestrationV2RpcSchemas.getThreadProjection.output),
);
const decodeIsoDateTime = Schema.decodeSync(IsoDateTime);

function isStreamEntry(rpcDef: WsRpcEntry): boolean {
  return (
    rpcDef.successSchema !== null &&
    (typeof rpcDef.successSchema === "object" || typeof rpcDef.successSchema === "function") &&
    "~effect/rpc/RpcSchema/StreamSchema" in rpcDef.successSchema
  );
}

const rpcEntries = Array.from(
  (
    WsRpcGroup as unknown as { readonly requests: ReadonlyMap<string, WsRpcEntry> }
  ).requests.entries(),
);

const compiledRpcCodecs = new Map<string, CompiledRpcCodecs>();
for (const [tag, rpcDef] of rpcEntries) {
  const targetSuccessSchema =
    isStreamEntry(rpcDef) && rpcDef.successSchema.success
      ? rpcDef.successSchema.success
      : rpcDef.successSchema;
  const targetErrorSchema =
    isStreamEntry(rpcDef) && rpcDef.successSchema.error
      ? rpcDef.successSchema.error
      : rpcDef.errorSchema;
  compiledRpcCodecs.set(tag, {
    decodePayload: Schema.decodeUnknownSync(Schema.toCodecJson(rpcDef.payloadSchema)),
    decodeSuccess: Schema.decodeUnknownSync(Schema.toCodecJson(targetSuccessSchema)),
    decodeError: Schema.decodeUnknownSync(Schema.toCodecJson(targetErrorSchema)),
  });
}

function getCompiledCodecs(tag: string): CompiledRpcCodecs {
  const codecs = compiledRpcCodecs.get(tag);
  if (!codecs) {
    throw new Error(`Missing compiled codecs for RPC "${tag}"`);
  }
  return codecs;
}

function assertValidPayload(tag: string, payload: unknown): void {
  try {
    getCompiledCodecs(tag).decodePayload(payload);
  } catch (err) {
    throw new Error(`Invalid test fixture payload for "${tag}": ${String(err)}`, { cause: err });
  }
}

function decodeSuccessOrChunk(tag: string, value: unknown): void {
  try {
    getCompiledCodecs(tag).decodeSuccess(value);
  } catch (err) {
    throw new Error(
      `Contract decode failed for "${tag}" (${JSON.stringify(value).slice(0, 400)}): ${String(err)}`,
      { cause: err },
    );
  }
}

function getExitCause(
  exit: NonNullable<ParityRpcFrame["exit"]>,
): ReadonlyArray<{ readonly _tag: string; readonly error?: unknown }> {
  return Array.isArray(exit.cause)
    ? (exit.cause as ReadonlyArray<{ readonly _tag: string; readonly error?: unknown }>)
    : [];
}

function assertAllowedFailure(
  tag: string,
  spec: RpcFixtureSpec,
  exit: NonNullable<ParityRpcFrame["exit"]>,
): void {
  const cause = getExitCause(exit);
  const first = cause[0];
  if (!first || first._tag !== "Fail") {
    throw new Error(`RPC "${tag}" failed with non-Fail cause: ${JSON.stringify(cause)}`);
  }
  const errObj = first.error as { readonly _tag?: string } | undefined;
  const errTag = errObj?._tag ?? "<unknown>";
  if (errTag === "UnsupportedRpcMethod") {
    throw new Error(`RPC "${tag}" returned UnsupportedRpcMethod`);
  }
  if (!spec.allowFailureTags?.includes(errTag)) {
    throw new Error(
      `RPC "${tag}" returned unexpected Failure tag "${errTag}": ${JSON.stringify(first.error)}`,
    );
  }
  try {
    getCompiledCodecs(tag).decodeError(first.error);
  } catch (err) {
    throw new Error(
      `Error schema decode failed for "${tag}" (${JSON.stringify(first.error)}): ${String(err)}`,
      { cause: err },
    );
  }
}

async function runSingleUnaryParity(
  rpc: ParityRpcHarness,
  reqId: string,
  tag: string,
  spec: RpcFixtureSpec,
): Promise<void> {
  rpc.sendRequest(reqId, tag, spec.payload);
  const frame = await rpc.nextFrameMatching(
    (f) => f._tag === "Defect" || (f._tag === "Exit" && f.requestId === reqId),
  );
  if (frame._tag === "Defect") {
    throw new Error(`Connection Defect while running "${tag}": ${JSON.stringify(frame.defect)}`);
  }
  if (!frame.exit) {
    throw new Error(`Missing exit on "${tag}"`);
  }
  if (frame.exit._tag === "Success") {
    decodeSuccessOrChunk(tag, frame.exit.value);
    return;
  }
  assertAllowedFailure(tag, spec, frame.exit);
}

async function runSingleStreamParity(
  rpc: ParityRpcHarness,
  reqId: string,
  probeId: string,
  tag: string,
  spec: RpcFixtureSpec,
): Promise<void> {
  let chunkCount = 0;
  let exited = false;
  let probeDone = false;
  let interruptedByClient = false;
  let streamError: Error | null = null;

  rpc.sendRequest(reqId, tag, spec.payload);
  rpc.sendRequest(probeId, "server.probe", {});

  const captureErr = (fn: () => void): void => {
    try {
      fn();
    } catch (err) {
      if (!streamError) {
        streamError = err instanceof Error ? err : new Error(String(err), { cause: err });
      }
    }
  };

  const handleStreamFrame = (frame: ParityRpcFrame): void => {
    if (frame._tag === "Defect") {
      if (!streamError) {
        streamError = new Error(
          `Connection Defect while streaming "${tag}": ${JSON.stringify(frame.defect)}`,
        );
      }
      exited = true;
      return;
    }
    if (frame._tag === "Chunk") {
      for (const value of frame.values ?? []) {
        captureErr(() => decodeSuccessOrChunk(tag, value));
        chunkCount += 1;
      }
      rpc.sendAck(reqId);
      return;
    }
    if (frame._tag === "Exit" && frame.exit) {
      exited = true;
      if (frame.exit._tag === "Failure") {
        const cause = getExitCause(frame.exit);
        if (interruptedByClient && cause[0]?._tag === "Interrupt") {
          return;
        }
        const failExit = frame.exit;
        captureErr(() => assertAllowedFailure(tag, spec, failExit));
      }
    }
  };

  const firstProbeWait = async (): Promise<void> => {
    const frame = await rpc.nextFrameMatching(
      (f) =>
        f._tag === "Defect" ||
        f.requestId === reqId ||
        (f._tag === "Exit" && f.requestId === probeId),
    );
    if (frame.requestId === probeId) {
      probeDone = true;
      return;
    }
    handleStreamFrame(frame);
  };

  // Barrier loop terminates strictly on `probeDone || exited || chunkCount > 0`
  const waitUntilBarrier = async (): Promise<void> => {
    await firstProbeWait();
    if (!exited && !probeDone && chunkCount === 0) {
      await waitUntilBarrier();
    }
  };
  await waitUntilBarrier();

  if (!exited && !spec.allowZeroChunks && chunkCount === 0 && !streamError) {
    const chunkFrame = await rpc.nextFrameMatching(
      (f) =>
        f._tag === "Defect" || (f.requestId === reqId && (f._tag === "Chunk" || f._tag === "Exit")),
    );
    handleStreamFrame(chunkFrame);
    if (!streamError && chunkCount < 1) {
      streamError = new Error(`Expected >= 1 chunk from stream "${tag}"`);
    }
  }

  if (!exited) {
    interruptedByClient = true;
    rpc.sendInterrupt(reqId);
    const drainExit = async (): Promise<void> => {
      const f = await rpc.nextFrameMatching((m) => m._tag === "Defect" || m.requestId === reqId);
      handleStreamFrame(f);
      if (!exited) {
        await drainExit();
      }
    };
    await drainExit();
  }

  if (streamError) {
    throw streamError;
  }
}

function seedGitRepoIfNeeded(dir: string, title = "# Parity Workspace\n"): void {
  NodeFS.mkdirSync(dir, { recursive: true });
  if (NodeFS.existsSync(NodePath.join(dir, ".git"))) {
    return;
  }
  NodeFS.writeFileSync(NodePath.join(dir, "README.md"), title);
  NodeChildProcess.execFileSync("git", ["init"], { cwd: dir, stdio: "ignore" });
  NodeChildProcess.execFileSync("git", ["config", "user.name", "Legion Parity"], {
    cwd: dir,
    stdio: "ignore",
  });
  NodeChildProcess.execFileSync("git", ["config", "user.email", "parity@legion.codes"], {
    cwd: dir,
    stdio: "ignore",
  });
  NodeChildProcess.execFileSync("git", ["add", "README.md"], { cwd: dir, stdio: "ignore" });
  NodeChildProcess.execFileSync("git", ["commit", "-m", "init"], { cwd: dir, stdio: "ignore" });
}

async function waitForThreadCondition(
  rpc: ParityRpcHarness,
  subId: string,
  threadId: string,
  check: () => Promise<boolean>,
): Promise<void> {
  rpc.sendRequest(subId, "orchestration.subscribeThread", {
    threadId,
    afterSequence: 0,
    requestCompletionMarker: true,
    acceptBoundedSnapshot: true,
    acceptDeferredWork: false,
    acceptCompactTurnItems: false,
    acceptDedupedBodies: false,
    acceptCompactIds: false,
  });

  try {
    const checkLoop = async (): Promise<void> => {
      await rpc.nextChunk(subId);
      if (await check()) {
        return;
      }
      await checkLoop();
    };
    if (!(await check())) {
      await checkLoop();
    }
  } finally {
    rpc.sendInterrupt(subId);
    await rpc.nextFrameMatching((f) => f._tag === "Exit" && f.requestId === subId);
  }
}

export async function runFullRpcAndProviderParityChecks(
  rpc: ParityRpcHarness,
  primaryWorkspaceDir: string,
  tempRoot: string,
  origin: string,
  primaryProjectId: string,
  primaryThreadId: string,
  isWindows: boolean,
): Promise<void> {
  const secondaryCwd = NodePath.join(tempRoot, "secondary-workspace");
  seedGitRepoIfNeeded(primaryWorkspaceDir, "# LegionCode Parity Workspace\n");
  seedGitRepoIfNeeded(secondaryCwd, "# Secondary Parity\n");
  // Modify README in primary workspace so review.getDiffPreview sees a real diff.
  NodeFS.writeFileSync(
    NodePath.join(primaryWorkspaceDir, "README.md"),
    "# LegionCode Parity Workspace\nModified line for review diff.\n",
  );

  let cmdSeq = 0;
  const nextCmd = (prefix = "wire-cmd"): string => `${prefix}-${++cmdSeq}`;

  await rpc.callUnary("seed-thread-parity", "orchestration.launchThread", {
    commandId: nextCmd("seed-thread"),
    threadId: primaryThreadId,
    projectId: primaryProjectId,
    title: "Primary Parity Thread",
    generateTitle: false,
    modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
    runtimeMode: "full-access",
    interactionMode: "default",
    workspaceStrategy: { type: "root" },
    initialMessage: { text: "Seed parity prompt", attachments: [] },
  });

  const ctx: ParityFixtureContext = {
    cwd: primaryWorkspaceDir,
    secondaryCwd,
    projectId: primaryProjectId,
    secondaryProjectId: "proj-wire-secondary",
    threadId: primaryThreadId,
    itemId: "item-wire-1",
    attachmentId: "att-wire-1",
    environmentId: "env-wire-1",
    taskId: "task-wire-1",
    terminalId: "term-wire-sweep",
    previewTabId: "tab-wire-1",
    serverUrl: origin,
    commandId: nextCmd,
  };

  // 1. Verify 100% WsRpcGroup.requests coverage across all methods.
  const fixtures = buildWsRpcFixtures(ctx);
  assert.strictEqual(
    Object.keys(fixtures).length,
    rpcEntries.length,
    "Fixture map must match WsRpcGroup.requests count",
  );

  const tested = new Set<string>();
  for (const [tag] of rpcEntries) {
    const spec = fixtures[tag];
    assert.isDefined(spec, `Missing fixture for WsRpcGroup method "${tag}"`);
    assertValidPayload(tag, spec.payload);
    tested.add(tag);
  }
  assert.strictEqual(tested.size, rpcEntries.length);

  // Execute in fixture order (CRUD dependencies line up: upsert before delete, open before close).
  // Collect all mismatches in a single pass so every Rust schema drift is surfaced at once.
  let seq = 0;
  const entryMap = new Map(rpcEntries);
  const sweepErrors: string[] = [];
  for (const [tag, spec] of Object.entries(fixtures)) {
    const rpcDef = entryMap.get(tag);
    assert.isDefined(rpcDef);
    const reqId = `sweep-${++seq}-${tag}`;
    try {
      if (isStreamEntry(rpcDef!)) {
        const probeId = `probe-${seq}-${tag}`;
        await runSingleStreamParity(rpc, reqId, probeId, tag, spec);
      } else {
        await runSingleUnaryParity(rpc, reqId, tag, spec);
      }
    } catch (err) {
      sweepErrors.push(`${tag}: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
  if (sweepErrors.length > 0) {
    throw new Error(
      `WsRpcGroup sweep had ${sweepErrors.length} failures:\n${sweepErrors.join("\n")}`,
    );
  }

  // Verify signed asset URLs minted by assets.createUrl decode via HTTP GET and emit favicon fallback marker.
  const workspaceAsset = (await rpc.callUnary("asset-ws-1", "assets.createUrl", {
    resource: { _tag: "workspace-file", threadId: primaryThreadId, path: "README.md" },
  })) as { readonly relativeUrl: string };
  const workspaceAssetRes = await fetch(`${origin}${workspaceAsset.relativeUrl}`);
  assert.strictEqual(workspaceAssetRes.status, 200);
  const workspaceAssetText = await workspaceAssetRes.text();
  assert.isTrue(workspaceAssetText.includes("Modified line for review diff."));

  const faviconFallback = (await rpc.callUnary("asset-fav-1", "assets.createUrl", {
    resource: { _tag: "project-favicon", cwd: primaryWorkspaceDir },
  })) as { readonly relativeUrl: string };
  assert.isTrue(faviconFallback.relativeUrl.endsWith("/project-favicon-missing"));

  // Verify live preview session tabId chaining works cleanly.
  const openedPreview = (await rpc.callUnary("prev-open-1", "preview.open", {
    threadId: primaryThreadId,
    url: `${origin}/`,
  })) as { readonly tabId: string };
  assert.isString(openedPreview.tabId);
  await rpc.callUnary("prev-resize-1", "preview.resize", {
    threadId: primaryThreadId,
    tabId: openedPreview.tabId,
    viewport: { _tag: "freeform", width: 1024, height: 768 },
  });
  await rpc.callUnary("prev-close-1", "preview.close", {
    threadId: primaryThreadId,
    tabId: openedPreview.tabId,
  });

  // 1b. Chained Project Workflow Board lifecycle parity (createCard -> updateSpec -> exploreStep -> challengeStep -> approveSpec -> updatePlan -> approvePlan -> returnForRevision -> approvePlan -> startStage -> getCard -> getArtifact -> invalidation).
  const createdCardRaw = await rpc.callUnary("wb-create-1", "workflowBoard.createCard", {
    projectId: primaryProjectId,
    title: "Wire Parity Board Card",
    brief: "Initial parity board brief",
    sourceType: "idea",
    assistanceMode: "balanced",
    intakeGoal: "change",
  });
  const createdCard = getCompiledCodecs("workflowBoard.createCard").decodeSuccess(
    createdCardRaw,
  ) as { readonly cardId: string; readonly revision: number };

  const updatedSpecRaw = await rpc.callUnary("wb-update-1", "workflowBoard.updateSpec", {
    cardId: createdCard.cardId,
    expectedRevision: createdCard.revision,
    acceptanceCriteria: [{ id: "AC-01", text: "Wire parity lifecycle decodes cleanly." }],
    constraints: ["No auto-commits"],
    sourceRefs: ["README.md"],
    wireframesIndex: null,
  });
  const updatedSpec = getCompiledCodecs("workflowBoard.updateSpec").decodeSuccess(
    updatedSpecRaw,
  ) as { readonly revision: number; readonly specDigest: string | null };
  assert.isString(updatedSpec.specDigest);

  const exploreInitRaw = await rpc.callUnary("wb-explore-1", "workflowBoard.exploreStep", {
    cardId: createdCard.cardId,
    expectedRevision: updatedSpec.revision,
    action: "step",
  });
  const exploreInit = getCompiledCodecs("workflowBoard.exploreStep").decodeSuccess(
    exploreInitRaw,
  ) as {
    readonly revision: number;
    readonly decisions: ReadonlyArray<{ readonly id: string; readonly status: string }>;
  };
  let exploreRevision = exploreInit.revision;
  const unresolvedIds = exploreInit.decisions
    .filter((decision) => decision.status === "unresolved")
    .map((decision) => decision.id);
  if (unresolvedIds.length > 0) {
    const exploreAnswerRaw = await rpc.callUnary("wb-explore-2", "workflowBoard.exploreStep", {
      cardId: createdCard.cardId,
      expectedRevision: exploreRevision,
      action: "step",
      answers: unresolvedIds.map((decisionId) => ({
        decisionId,
        action: "recommend",
      })),
    });
    const exploreAnswered = getCompiledCodecs("workflowBoard.exploreStep").decodeSuccess(
      exploreAnswerRaw,
    ) as { readonly revision: number };
    exploreRevision = exploreAnswered.revision;
  }

  const challengeGenRaw = await rpc.callUnary("wb-challenge-1", "workflowBoard.challengeStep", {
    cardId: createdCard.cardId,
    expectedRevision: exploreRevision,
    action: "start",
    concerns: [
      {
        id: "conc-parity-1",
        summary: "Confirm parity harness isolates workspace writes",
        whyMatters: "Prevents untracked edits leaking",
        evidence: "README.md",
        assumption: "All board writes are journaled",
        disposition: "unresolved",
      },
    ],
  });
  const challengeGen = getCompiledCodecs("workflowBoard.challengeStep").decodeSuccess(
    challengeGenRaw,
  ) as { readonly revision: number };

  const challengeAnsRaw = await rpc.callUnary("wb-challenge-2", "workflowBoard.challengeStep", {
    cardId: createdCard.cardId,
    expectedRevision: challengeGen.revision,
    action: "resolve",
    concerns: [
      {
        id: "conc-parity-1",
        summary: "Confirm parity harness isolates workspace writes",
        disposition: "answer",
        answer: "Board writes go through execute_journaled_writes without auto-commits.",
      },
    ],
  });
  const challengeAns = getCompiledCodecs("workflowBoard.challengeStep").decodeSuccess(
    challengeAnsRaw,
  ) as { readonly revision: number; readonly challengeReceipt: { readonly status: string } };
  assert.strictEqual(challengeAns.challengeReceipt.status, "resolved");

  const approvedSpecRaw = await rpc.callUnary("wb-approve-1", "workflowBoard.approveSpec", {
    cardId: createdCard.cardId,
    expectedRevision: challengeAns.revision,
    expectedDigest: updatedSpec.specDigest ?? undefined,
  });
  const approvedSpec = getCompiledCodecs("workflowBoard.approveSpec").decodeSuccess(
    approvedSpecRaw,
  ) as { readonly revision: number; readonly specApproval: { readonly status: string } };
  assert.strictEqual(approvedSpec.specApproval.status, "valid");

  const planPayload = {
    cardId: createdCard.cardId,
    expectedRevision: approvedSpec.revision,
    lenses: [
      { lens: "ux", applicable: false, reason: "CLI parity flow only" },
      { lens: "functional", applicable: true, summary: "Covers AC-01 wire round-trip" },
      { lens: "architecture", applicable: true, summary: "Rust board service + TS codecs" },
      { lens: "quality", applicable: true, summary: "Verified by rustWireParity" },
      { lens: "infrastructure", applicable: false, reason: "Local SQLite store only" },
      { lens: "delivery", applicable: true, summary: "Single balanced task" },
    ],
    alternatives: [
      {
        id: "alt-a",
        title: "Single-pass typed codecs",
        summary: "Decode Rust board responses directly through Effect Schema codecs.",
        tradeoffs: "Requires lockstep TS and Rust contracts.",
      },
      {
        id: "alt-b",
        title: "Untyped JSON pass-through",
        summary: "Skip schema validation for board RPC responses.",
        tradeoffs: "Loses wire parity guarantees.",
      },
    ],
    selectedAlternativeId: "alt-a",
    selectionRationale: "Lockstep typed codecs catch wire drift immediately.",
    strategy: "outcomes",
    strategyRationale: "Validate AC-01 end-to-end.",
    granularity: "balanced",
    tasks: [
      {
        taskId: "TSK-01",
        title: "Wire parity task",
        filesAllowed: ["README.md"],
        filesForbidden: [".env"],
        expectedArtifacts: ["README.md"],
        verificationCommands: [["git", "status", "--short"]],
        testingMethod: "regression-first",
      },
    ],
    checks: [
      {
        id: "CHK-01",
        description: "Git status check",
        argv: ["git", "status", "--short"],
      },
    ],
    acMappings: [
      {
        criterionId: "AC-01",
        method: "automated",
        checkIds: ["CHK-01"],
      },
    ],
  };

  const updatedPlanRaw = await rpc.callUnary("wb-plan-1", "workflowBoard.updatePlan", planPayload);
  const updatedPlan = getCompiledCodecs("workflowBoard.updatePlan").decodeSuccess(
    updatedPlanRaw,
  ) as { readonly revision: number; readonly planDigest: string | null };
  assert.isString(updatedPlan.planDigest);

  const approvedPlanRaw = await rpc.callUnary("wb-approve-plan-1", "workflowBoard.approvePlan", {
    cardId: createdCard.cardId,
    expectedRevision: updatedPlan.revision,
    expectedSpecDigest: updatedSpec.specDigest ?? undefined,
    expectedPlanDigest: updatedPlan.planDigest ?? undefined,
  });
  const approvedPlan = getCompiledCodecs("workflowBoard.approvePlan").decodeSuccess(
    approvedPlanRaw,
  ) as { readonly revision: number; readonly planApproval: { readonly status: string } };
  assert.strictEqual(approvedPlan.planApproval.status, "valid");

  const returnedRaw = await rpc.callUnary("wb-return-1", "workflowBoard.returnForRevision", {
    cardId: createdCard.cardId,
    expectedRevision: approvedPlan.revision,
    targetStage: "plan",
    reason: "Exercise returnForRevision invalidation parity",
  });
  const returned = getCompiledCodecs("workflowBoard.returnForRevision").decodeSuccess(
    returnedRaw,
  ) as {
    readonly revision: number;
    readonly specApproval: { readonly status: string } | null;
    readonly planApproval: { readonly status: string } | null;
  };
  assert.strictEqual(returned.specApproval?.status, "valid");
  assert.strictEqual(returned.planApproval?.status, "stale");

  const reapprovedPlanRaw = await rpc.callUnary("wb-approve-plan-2", "workflowBoard.approvePlan", {
    cardId: createdCard.cardId,
    expectedRevision: returned.revision,
    expectedSpecDigest: updatedSpec.specDigest ?? undefined,
    expectedPlanDigest: updatedPlan.planDigest ?? undefined,
  });
  const reapprovedPlan = getCompiledCodecs("workflowBoard.approvePlan").decodeSuccess(
    reapprovedPlanRaw,
  ) as { readonly revision: number; readonly planApproval: { readonly status: string } };
  assert.strictEqual(reapprovedPlan.planApproval.status, "valid");

  await rpc.callUnary("wb-enable-wf", "server.updateSettings", {
    patch: { workflowsEnabled: true },
  });
  try {
    const start1Raw = await rpc.callUnary("wb-start-1", "workflowBoard.startStage", {
      cardId: createdCard.cardId,
      expectedRevision: reapprovedPlan.revision,
      stage: "plan",
      modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      runtimeMode: "full-access",
      interactionMode: "default",
      workspaceStrategy: { type: "root" },
    });
    const start1 = getCompiledCodecs("workflowBoard.startStage").decodeSuccess(start1Raw) as {
      readonly revision: number;
      readonly threadId: string;
      readonly runId: string | null;
      readonly resumed: boolean;
    };
    assert.isFalse(start1.resumed);
    assert.isNotNull(start1.runId);

    const start2Raw = await rpc.callUnary("wb-start-2", "workflowBoard.startStage", {
      cardId: createdCard.cardId,
      expectedRevision: start1.revision,
      stage: "plan",
      modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      runtimeMode: "full-access",
      interactionMode: "default",
      workspaceStrategy: { type: "root" },
    });
    const start2 = getCompiledCodecs("workflowBoard.startStage").decodeSuccess(start2Raw) as {
      readonly threadId: string;
      readonly resumed: boolean;
    };
    assert.isTrue(start2.resumed);
    assert.strictEqual(start2.threadId, start1.threadId);

    const cardDetailRaw = await rpc.callUnary("wb-get-1", "workflowBoard.getCard", {
      cardId: createdCard.cardId,
      projectId: primaryProjectId,
    });
    getCompiledCodecs("workflowBoard.getCard").decodeSuccess(cardDetailRaw);

    const specArtifactRaw = await rpc.callUnary("wb-art-spec", "workflowBoard.getArtifact", {
      cardId: createdCard.cardId,
      key: "spec.md",
    });
    getCompiledCodecs("workflowBoard.getArtifact").decodeSuccess(specArtifactRaw);

    const planArtifactRaw = await rpc.callUnary("wb-art-plan", "workflowBoard.getArtifact", {
      cardId: createdCard.cardId,
      key: "plan.md",
    });
    getCompiledCodecs("workflowBoard.getArtifact").decodeSuccess(planArtifactRaw);

    const invalidatedSpecRaw = await rpc.callUnary("wb-update-2", "workflowBoard.updateSpec", {
      cardId: createdCard.cardId,
      expectedRevision: start1.revision,
      brief: "Edited brief triggers targeted Spec and Plan invalidation.",
    });
    const invalidatedSpec = getCompiledCodecs("workflowBoard.updateSpec").decodeSuccess(
      invalidatedSpecRaw,
    ) as {
      readonly revision: number;
      readonly specApproval: { readonly status: string } | null;
      readonly planApproval: { readonly status: string } | null;
    };
    assert.strictEqual(invalidatedSpec.specApproval?.status, "stale");
    assert.strictEqual(invalidatedSpec.planApproval?.status, "stale");

    const cardDiffRaw = await rpc.callUnary("wb-diff-1", "workflowBoard.getCardDiff", {
      cardId: createdCard.cardId,
    });
    getCompiledCodecs("workflowBoard.getCardDiff").decodeSuccess(cardDiffRaw);

    const pausedRaw = await rpc.callUnary("wb-pause-1", "workflowBoard.pauseCard", {
      cardId: createdCard.cardId,
      expectedRevision: invalidatedSpec.revision,
      reason: "Parity pause",
    });
    const paused = getCompiledCodecs("workflowBoard.pauseCard").decodeSuccess(pausedRaw) as {
      readonly revision: number;
      readonly status: string;
    };
    assert.strictEqual(paused.status, "paused");

    const resumedRaw = await rpc.callUnary("wb-resume-1", "workflowBoard.resumeCard", {
      cardId: createdCard.cardId,
      expectedRevision: paused.revision,
      reconcileInterrupted: true,
    });
    const resumed = getCompiledCodecs("workflowBoard.resumeCard").decodeSuccess(resumedRaw) as {
      readonly revision: number;
    };

    const amendedRaw = await rpc.callUnary("wb-amend-1", "workflowBoard.proposeAmendment", {
      cardId: createdCard.cardId,
      expectedRevision: resumed.revision,
      targetStage: "spec",
      reason: "Parity amendment probe",
    });
    getCompiledCodecs("workflowBoard.proposeAmendment").decodeSuccess(amendedRaw);
  } finally {
    await rpc.callUnary("wb-restore-wf", "server.updateSettings", {
      patch: { workflowsEnabled: false },
    });
  }

  // 1c. Chained M4 CLI Project Import wire parity (previewCliImport -> applyCliImport -> getCard).
  const cliParityRel = ".legion-cli-parity";
  const cliParityRoot = NodePath.join(primaryWorkspaceDir, cliParityRel);
  NodeFS.mkdirSync(NodePath.join(cliParityRoot, "workflow"), { recursive: true });
  NodeFS.mkdirSync(NodePath.join(cliParityRoot, "specs", "spec-wire-parity"), { recursive: true });
  NodeFS.mkdirSync(NodePath.join(cliParityRoot, "tasks"), { recursive: true });
  NodeFS.writeFileSync(
    NodePath.join(cliParityRoot, "config.yaml"),
    "schemaVersion: legion-cli-config/v1\nsandbox:\n  requireHardened: true\n",
  );
  NodeFS.writeFileSync(
    NodePath.join(cliParityRoot, "workflow", "execution.yaml"),
    [
      "schemaVersion: legion-cli-workflow-evidence/v1",
      "specId: spec-wire-parity",
      "status: blocked",
      `blocker: "verification command failed with exit 1: node --test test.mjs (log: ${cliParityRel}/cache/runs/r1/verify-1.log)"`,
      "updatedAt: 2026-10-22T02:28:41.128Z",
      "",
    ].join("\n"),
  );
  NodeFS.writeFileSync(
    NodePath.join(cliParityRoot, "workflow", "spec-approval.yaml"),
    [
      "schemaVersion: legion-cli-spec-approval/v1",
      "specId: spec-wire-parity",
      "approvedAt: 2026-10-22T01:00:17.476Z",
      "approvedBy: user",
      "",
    ].join("\n"),
  );
  NodeFS.writeFileSync(
    NodePath.join(cliParityRoot, "workflow", "plan-approval.yaml"),
    [
      "schemaVersion: legion-cli-plan-approval/v1",
      "specId: spec-wire-parity",
      "approvedAt: 2026-10-22T01:30:41.084Z",
      "approvedBy: user",
      "taskIds:",
      "  - TSK-0001",
      "acceptanceIds:",
      "  - AC-01",
      "",
    ].join("\n"),
  );
  NodeFS.writeFileSync(
    NodePath.join(cliParityRoot, "specs", "spec-wire-parity", "SPEC.md"),
    "---\nid: spec-wire-parity\ntitle: Wire Parity Spec\n---\n\n- AC-01: Parity decodes cleanly\n",
  );
  NodeFS.writeFileSync(
    NodePath.join(cliParityRoot, "tasks", "TSK-0001.md"),
    [
      "---",
      "schemaVersion: legion-cli-task/v1",
      "id: TSK-0001",
      "title: Parity Task",
      "status: blocked",
      "specId: spec-wire-parity",
      "contract:",
      "  filesAllowed:",
      "    - src/parity.ts",
      "  verificationCommands:",
      "    - node --test test.mjs",
      "---",
      "",
      "Parity task.",
      "",
    ].join("\n"),
  );

  const previewCliRaw = await rpc.callUnary("wb-cli-preview-1", "workflowBoard.previewCliImport", {
    projectId: primaryProjectId,
    cliDir: cliParityRel,
  });
  const previewCli = getCompiledCodecs("workflowBoard.previewCliImport").decodeSuccess(
    previewCliRaw,
  ) as {
    readonly specId: string;
    readonly canApply: boolean;
    readonly tasks: ReadonlyArray<unknown>;
    readonly unsupportedPolicies: ReadonlyArray<unknown>;
    readonly missingLogNotes: ReadonlyArray<unknown>;
  };
  assert.strictEqual(previewCli.specId, "spec-wire-parity");
  assert.isTrue(previewCli.canApply);
  assert.strictEqual(previewCli.tasks.length, 1);
  assert.isAtLeast(previewCli.unsupportedPolicies.length, 1);
  assert.isAtLeast(previewCli.missingLogNotes.length, 1);

  const applyCliRaw = await rpc.callUnary("wb-cli-apply-1", "workflowBoard.applyCliImport", {
    projectId: primaryProjectId,
    cliDir: cliParityRel,
    acknowledgeUnsupportedPolicies: true,
  });
  const applyCli = getCompiledCodecs("workflowBoard.applyCliImport").decodeSuccess(applyCliRaw) as {
    readonly cardId: string;
    readonly created: boolean;
    readonly sourceSpecId: string;
  };
  assert.isTrue(applyCli.created);
  assert.strictEqual(applyCli.sourceSpecId, "spec-wire-parity");

  const importedDetailRaw = await rpc.callUnary("wb-cli-get-1", "workflowBoard.getCard", {
    cardId: applyCli.cardId,
    projectId: primaryProjectId,
  });
  getCompiledCodecs("workflowBoard.getCard").decodeSuccess(importedDetailRaw);

  // 2. launchThread (generateTitle: true + initialMessage) + titleRegeneration + settle/unsettle.
  const titleThreadId = "thread-parity-title";
  await rpc.callUnary("launch-title-1", "orchestration.launchThread", {
    commandId: ctx.commandId("launch-title"),
    threadId: titleThreadId,
    projectId: primaryProjectId,
    title: "New thread",
    generateTitle: true,
    modelSelection: { instanceId: "devReplay", model: "replay-default", options: [] },
    runtimeMode: "full-access",
    interactionMode: "default",
    workspaceStrategy: { type: "root" },
    initialMessage: {
      text: "Explain Rust ownership and borrowing concisely",
      attachments: [],
    },
  });

  await waitForThreadCondition(rpc, "sub-title-1", titleThreadId, async () => {
    const raw = await rpc.callUnary(
      ctx.commandId("proj-title"),
      "orchestration.getThreadProjection",
      {
        threadId: titleThreadId,
      },
    );
    const decoded = decodeThreadProjection(raw);
    const runCompleted = decoded.runs.some((r) => r.status === "completed");
    return (
      runCompleted &&
      decoded.thread.titleRegeneration === null &&
      decoded.thread.lastTitleRegeneration !== null
    );
  });

  await rpc.callUnary("cmd-settle-1", "orchestration.dispatchCommand", {
    type: "thread.settle",
    commandId: ctx.commandId("settle"),
    threadId: titleThreadId,
  });
  await rpc.callUnary("cmd-unsettle-1", "orchestration.dispatchCommand", {
    type: "thread.unsettle",
    commandId: ctx.commandId("unsettle"),
    threadId: titleThreadId,
    reason: "user",
  });
  const unsettledRaw = await rpc.callUnary(
    "proj-unsettled-1",
    "orchestration.getThreadProjection",
    {
      threadId: titleThreadId,
    },
  );
  const unsettledProj = decodeThreadProjection(unsettledRaw);
  assert.strictEqual(unsettledProj.thread.settledOverride, "active");

  // 3. Configure mock subprocess provider instances (Claude, Codex, Antigravity ACP, Failing).
  const scripts = writeMockProviderScripts(NodePath.join(tempRoot, "mock-providers"), isWindows);
  await rpc.callUnary("settings-mocks-1", "server.updateSettings", {
    patch: {
      providerInstances: {
        "mock-claude": {
          driver: "claudeAgent",
          enabled: true,
          config: { binaryPath: scripts.claudeBinaryPath },
        },
        "mock-codex": {
          driver: "codex",
          enabled: true,
          config: { binaryPath: scripts.codexBinaryPath },
        },
        "mock-acp": {
          driver: "antigravity",
          enabled: true,
          config: { binaryPath: scripts.acpBinaryPath },
        },
        "mock-fail": {
          driver: "claudeAgent",
          enabled: true,
          config: { binaryPath: scripts.failBinaryPath },
        },
      },
    },
  });

  const runMockTurn = async (
    threadId: string,
    instanceId: string,
    model: string,
    expectedStatus: "completed" | "failed",
  ) => {
    await rpc.callUnary(`launch-${instanceId}`, "orchestration.launchThread", {
      commandId: ctx.commandId(`launch-${instanceId}`),
      threadId,
      projectId: primaryProjectId,
      title: `Mock ${instanceId}`,
      generateTitle: false,
      modelSelection: { instanceId, model, options: [] },
      runtimeMode: "full-access",
      interactionMode: "default",
      workspaceStrategy: { type: "root" },
      initialMessage: { text: `Prompt for ${instanceId}`, attachments: [] },
    });

    let latest = decodeThreadProjection(
      await rpc.callUnary(ctx.commandId("proj"), "orchestration.getThreadProjection", { threadId }),
    );
    await waitForThreadCondition(rpc, `sub-${instanceId}`, threadId, async () => {
      latest = decodeThreadProjection(
        await rpc.callUnary(ctx.commandId("proj"), "orchestration.getThreadProjection", {
          threadId,
        }),
      );
      return latest.runs.some((r) => r.status === expectedStatus);
    });
    return latest;
  };

  // Claude stream-json: verifies deduplication + TurnTokenUsage.
  const claudeProj = await runMockTurn(
    "thread-mock-claude",
    "mock-claude",
    "claude-sonnet-4-5",
    "completed",
  );
  const claudeAsst = claudeProj.messages
    .filter((m) => m.role === "assistant")
    .map((m) => m.text)
    .join("");
  assert.strictEqual(claudeAsst.match(/Hello from Claude mock\./g)?.length, 1);
  assert.isTrue(
    claudeProj.providerTurns.some(
      (t) =>
        t.turnTokenUsage?.usageStatus === "complete" &&
        t.turnTokenUsage.inputTokens === 12 &&
        t.turnTokenUsage.outputTokens === 7,
    ),
  );

  // Codex app-server JSON-RPC mock.
  const codexProj = await runMockTurn("thread-mock-codex", "mock-codex", "gpt-5", "completed");
  assert.include(
    codexProj.messages
      .filter((m) => m.role === "assistant")
      .map((m) => m.text)
      .join(""),
    "Hello from Codex mock.",
  );

  // ACP JSON-RPC mock (antigravity driver).
  const acpProj = await runMockTurn("thread-mock-acp", "mock-acp", "gemini-2.5-pro", "completed");
  assert.include(
    acpProj.messages
      .filter((m) => m.role === "assistant")
      .map((m) => m.text)
      .join(""),
    "Hello from ACP mock.",
  );

  // Failing subprocess -> RunStatus::Failed + "error" TurnItem.
  const failProj = await runMockTurn(
    "thread-mock-fail",
    "mock-fail",
    "claude-sonnet-4-5",
    "failed",
  );
  assert.isTrue(failProj.turnItems.some((item) => item.type === "error"));

  // 4. Verify v0.1.12 HTTP endpoints (/oauth/mcp/revoke, /api/peer-link/client-pairing, /api/links/thread/*).
  const oauthMetaRes = await fetch(`${origin}/.well-known/oauth-authorization-server`);
  assert.strictEqual(oauthMetaRes.status, 200);
  const oauthMeta = (await oauthMetaRes.json()) as { readonly revocation_endpoint?: string };
  assert.isTrue(oauthMeta.revocation_endpoint?.endsWith("/oauth/mcp/revoke") === true);

  const revokeRes = await fetch(`${origin}/oauth/mcp/revoke`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: "token=nonexistent-parity-token",
  });
  assert.strictEqual(revokeRes.status, 200);
  assert.strictEqual(revokeRes.headers.get("cache-control"), "no-store");

  const unauthPairingRes = await fetch(`${origin}/api/peer-link/client-pairing`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ scopes: ["orchestration:read"], clientLabel: "Parity" }),
  });
  assert.strictEqual(unauthPairingRes.status, 401);

  const originPairingRes = await fetch(`${origin}/api/peer-link/client-pairing`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      origin: "http://evil.example",
    },
    body: JSON.stringify({ scopes: ["orchestration:read"], clientLabel: "Parity" }),
  });
  assert.strictEqual(originPairingRes.status, 403);

  for (const peerThreadPath of [
    `${PEER_LINK_THREAD_STREAM_ROUTE}?thread=${encodeURIComponent(primaryThreadId)}`,
    `${PEER_LINK_THREAD_HISTORY_ROUTE}?thread=${encodeURIComponent(primaryThreadId)}&cursor=1`,
    `${PEER_LINK_THREAD_ITEM_ROUTE}?thread=${encodeURIComponent(primaryThreadId)}&item=${encodeURIComponent(ctx.itemId)}`,
  ]) {
    const peerRes = await fetch(`${origin}${peerThreadPath}`);
    assert.strictEqual(peerRes.status, 401);
  }

  // 5. Exercise Wave 1/7 shared & contract helpers.
  const normalizedCallback = pastedUrl("127.0.0.1:1455/cb?code=abc&state=s1");
  assert.strictEqual(normalizedCallback.origin, "http://127.0.0.1:1455");

  const sampleMilestone = {
    id: MilestoneId.make("m-1"),
    occurredAt: decodeIsoDateTime("2026-01-01T00:00:00.000Z"),
    type: "thread.run_settled" as const,
    projectId: ProjectId.make(ctx.projectId),
    threadId: ThreadId.make(ctx.threadId),
    parentThreadId: null,
    actor: { type: "agent" as const },
    isSubagentChild: false,
    runId: RunId.make("run-1"),
    outcome: "completed" as const,
  };
  assert.isTrue(
    milestoneMatches(sampleMilestone, {
      types: ["thread.run_settled"],
      projectIds: [ProjectId.make(ctx.projectId)],
    }),
  );
  assert.isFalse(
    milestoneMatches(sampleMilestone, { workflows: [TrimmedNonEmptyString.make("nightly")] }),
  );

  const claudeProvider = ProviderInstanceId.make("claudeAgent");
  assert.isFalse(hasPendingBackgroundSubagent([], claudeProvider));
  assert.isTrue(
    hasPendingBackgroundSubagent(
      [
        {
          kind: "subagent" as const,
          taskId: TrimmedNonEmptyString.make("bg-1"),
          providerInstanceId: claudeProvider,
        },
      ],
      claudeProvider,
    ),
  );
}

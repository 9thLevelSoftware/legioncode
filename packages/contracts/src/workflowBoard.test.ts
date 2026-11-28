import * as Exit from "effect/Exit";
import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import { AuthOrchestrationOperateScope, AuthOrchestrationReadScope } from "./auth.ts";
import { CLIENT_GUARDED_RPC_SCOPES, clientRpcRequiredScopes } from "./clientRpcPermissions.ts";
import { WsRpcGroup } from "./rpc.ts";
import {
  ProjectWorkflowBoardSnapshot,
  WorkflowBoardApprovalId,
  WorkflowBoardApprovalStatus,
  WorkflowBoardApproveSpecInput,
  WorkflowBoardArtifact,
  WorkflowBoardArtifactKind,
  WorkflowBoardAssistanceMode,
  WorkflowBoardCardStatus,
  WorkflowBoardCardSummary,
  WorkflowBoardCreateCardInput,
  WorkflowBoardGetArtifactInput,
  WorkflowBoardRequestError,
  WorkflowBoardSpecSourceType,
  WorkflowBoardStage,
  WorkflowBoardStageRunId,
  WorkflowBoardStageRunStatus,
  WorkflowBoardStartStageInput,
  WorkflowBoardStartStageResult,
  WorkflowBoardSubscribeInput,
  WorkflowBoardUpdateSpecInput,
  WorkflowBoardUpdateSpecResult,
  WorkflowCardDetail,
  WorkflowCardId,
} from "./workflowBoard.ts";
import { WorkflowBoardSafeInputPath } from "./workflowBoardPlanning.ts";

const decodeCardId = Schema.decodeSync(WorkflowCardId);
const decodeCardIdExit = Schema.decodeExit(WorkflowCardId);
const decodeApprovalId = Schema.decodeSync(WorkflowBoardApprovalId);
const decodeApprovalIdExit = Schema.decodeExit(WorkflowBoardApprovalId);
const decodeStageRunId = Schema.decodeSync(WorkflowBoardStageRunId);
const decodeStageRunIdExit = Schema.decodeExit(WorkflowBoardStageRunId);

const decodeStageExit = Schema.decodeExit(WorkflowBoardStage);
const decodeStageUnknownExit = Schema.decodeUnknownExit(WorkflowBoardStage);
const decodeCardStatusExit = Schema.decodeExit(WorkflowBoardCardStatus);
const decodeCardStatusUnknownExit = Schema.decodeUnknownExit(WorkflowBoardCardStatus);
const decodeSpecSourceTypeExit = Schema.decodeExit(WorkflowBoardSpecSourceType);
const decodeSpecSourceTypeUnknownExit = Schema.decodeUnknownExit(WorkflowBoardSpecSourceType);
const decodeAssistanceModeExit = Schema.decodeExit(WorkflowBoardAssistanceMode);
const decodeAssistanceModeUnknownExit = Schema.decodeUnknownExit(WorkflowBoardAssistanceMode);
const decodeApprovalStatusExit = Schema.decodeExit(WorkflowBoardApprovalStatus);
const decodeApprovalStatusUnknownExit = Schema.decodeUnknownExit(WorkflowBoardApprovalStatus);
const decodeArtifactKindExit = Schema.decodeExit(WorkflowBoardArtifactKind);
const decodeStageRunStatusExit = Schema.decodeExit(WorkflowBoardStageRunStatus);

const snapshotCodec = Schema.toCodecJson(ProjectWorkflowBoardSnapshot);
const decodeSnapshot = Schema.decodeSync(snapshotCodec);
const encodeSnapshot = Schema.encodeSync(snapshotCodec);

const detailCodec = Schema.toCodecJson(WorkflowCardDetail);
const decodeDetail = Schema.decodeSync(detailCodec);
const encodeDetail = Schema.encodeSync(detailCodec);

const decodeGetArtifact = Schema.decodeUnknownExit(WorkflowBoardGetArtifactInput);
const decodeSafeInputPath = Schema.decodeUnknownExit(WorkflowBoardSafeInputPath);
const artifactCodec = Schema.toCodecJson(WorkflowBoardArtifact);
const decodeArtifact = Schema.decodeSync(artifactCodec);
const encodeArtifact = Schema.encodeSync(artifactCodec);

const decodeUpdateSpecUnknownExit = Schema.decodeUnknownExit(WorkflowBoardUpdateSpecInput);
const decodeUpdateSpecSync = Schema.decodeSync(WorkflowBoardUpdateSpecInput);
const decodeApproveSpecUnknownExit = Schema.decodeUnknownExit(WorkflowBoardApproveSpecInput);
const decodeStartStageUnknownExit = Schema.decodeUnknownExit(WorkflowBoardStartStageInput);

const decodeSubscribeSync = Schema.decodeSync(WorkflowBoardSubscribeInput);
const decodeCreateCardSync = Schema.decodeSync(WorkflowBoardCreateCardInput);

const startStageResultCodec = Schema.toCodecJson(WorkflowBoardStartStageResult);
const decodeStartStageResult = Schema.decodeSync(startStageResultCodec);
const encodeStartStageResult = Schema.encodeSync(startStageResultCodec);

const errCodec = Schema.toCodecJson(WorkflowBoardRequestError);
const decodeErr = Schema.decodeSync(errCodec);
const encodeErr = Schema.encodeSync(errCodec);

const decodeUpdateResultUnknownExit = Schema.decodeUnknownExit(WorkflowBoardUpdateSpecResult);
const decodeUpdateResultSync = Schema.decodeSync(WorkflowBoardUpdateSpecResult);

describe("workflowBoard contracts", () => {
  it("validates and trims branded IDs while rejecting empty or blank values", () => {
    expect(decodeCardId("  card-123  ")).toBe("card-123");
    expect(decodeApprovalId(" apr-1 ")).toBe("apr-1");
    expect(decodeStageRunId(" sr-1 ")).toBe("sr-1");

    for (const bad of ["", "   ", "\t\n"]) {
      expect(Exit.isFailure(decodeCardIdExit(bad))).toBe(true);
      expect(Exit.isFailure(decodeApprovalIdExit(bad))).toBe(true);
      expect(Exit.isFailure(decodeStageRunIdExit(bad))).toBe(true);
    }
  });

  it("enforces stage, status, sourceType, assistanceMode, approval, artifactKind, and stageRunStatus enums", () => {
    expect(Exit.isSuccess(decodeStageExit("spec"))).toBe(true);
    expect(Exit.isSuccess(decodeStageExit("ship"))).toBe(true);
    expect(Exit.isFailure(decodeStageUnknownExit("review"))).toBe(true);

    expect(Exit.isSuccess(decodeCardStatusExit("blocked"))).toBe(true);
    expect(Exit.isFailure(decodeCardStatusUnknownExit("archived"))).toBe(true);

    expect(Exit.isSuccess(decodeSpecSourceTypeExit("codebase"))).toBe(true);
    expect(Exit.isFailure(decodeSpecSourceTypeUnknownExit("brownfield"))).toBe(true);

    expect(Exit.isSuccess(decodeAssistanceModeExit("balanced"))).toBe(true);
    expect(Exit.isFailure(decodeAssistanceModeUnknownExit("auto"))).toBe(true);

    for (const status of ["valid", "stale", "superseded", "historical"] as const) {
      expect(Exit.isSuccess(decodeApprovalStatusExit(status))).toBe(true);
    }
    expect(Exit.isFailure(decodeApprovalStatusUnknownExit("none"))).toBe(true);

    for (const kind of ["card", "spec", "plan", "design", "wireframes"] as const) {
      expect(Exit.isSuccess(decodeArtifactKindExit(kind))).toBe(true);
    }

    for (const status of [
      "pending",
      "launching",
      "running",
      "paused",
      "completed",
      "failed",
      "cancelled",
    ] as const) {
      expect(Exit.isSuccess(decodeStageRunStatusExit(status))).toBe(true);
    }
  });

  it("round-trips compact snapshot and full card detail via JSON codec matching Rust wire fixtures", () => {
    const summaryJson = {
      cardId: "card-1",
      projectId: "proj-1",
      title: "Add project workflow board",
      stage: "plan",
      status: "active",
      revision: 3,
      currentTask: null,
      blockerSummary: null,
      nextAction: "start_plan",
      specApprovedRevision: 3,
      specApprovalStatus: "valid",
      activeThreadId: "thr-1",
      createdAt: "2026-11-24T00:00:00Z",
      updatedAt: "2026-11-24T01:00:00Z",
    } as const;

    const snapshotWire = {
      projectId: "proj-1",
      revision: 3,
      cards: [summaryJson],
      generatedAt: "2026-11-24T01:00:00Z",
    };
    const decodedSnapshot = decodeSnapshot(snapshotWire);
    expect(encodeSnapshot(decodedSnapshot)).toEqual(snapshotWire);
    expect(Object.keys(WorkflowBoardCardSummary.fields)).not.toContain("spec");
    expect(Object.keys(WorkflowBoardCardSummary.fields)).not.toContain("diff");
    expect(Object.keys(WorkflowBoardCardSummary.fields)).not.toContain("transcript");

    const detailWire = {
      ...summaryJson,
      workspaceStrategy: {
        type: "existing_worktree",
        worktreePath: "/repo/.worktrees/card-1",
        branch: "feat/card-1",
      },
      modelSelection: {
        instanceId: "devReplay",
        model: "dev-replay",
      },
      runtimeMode: "auto-accept-edits",
      interactionMode: "plan",
      spec: {
        title: "Add project workflow board",
        brief: "Ship a 4-stage board",
        sourceType: "idea",
        assistanceMode: "balanced",
        acceptanceCriteria: [{ id: "AC-01", text: "Card persists across restart" }],
        constraints: ["No auto-commits"],
        sourceRefs: ["README.md"],
        wireframesIndex: null,
        specDigest: "sha256:abc123",
        updatedAt: "2026-11-24T00:30:00Z",
      },
      specApproval: {
        id: "apr-1",
        gate: "spec",
        status: "valid",
        revision: 3,
        artifactDigest: "sha256:abc123",
        specDigest: "sha256:abc123",
        wireframesDigest: null,
        approvedAt: "2026-11-24T00:45:00Z",
        approvedBy: "client",
      },
      stageRuns: [
        {
          id: "sr-1",
          stage: "plan",
          stableLaunchKey: "card-1:plan:1",
          status: "running",
          cardRevision: 3,
          runId: "run-1",
          threadId: "thr-1",
          projectId: "proj-1",
          workspaceStrategy: { type: "root" },
          worktreePath: "/repo/.worktrees/card-1",
          branch: "feat/card-1",
          modelSelection: { instanceId: "devReplay", model: "dev-replay" },
          runtimeMode: "auto-accept-edits",
          interactionMode: "plan",
          lastError: null,
          createdAt: "2026-11-24T00:50:00Z",
          updatedAt: "2026-11-24T01:00:00Z",
          endedAt: null,
        },
      ],
      revisions: [
        {
          revision: 1,
          stage: "spec",
          kind: "card_created",
          summary: "Created card",
          specDigest: null,
          createdAt: "2026-11-24T00:00:00Z",
        },
      ],
      artifactKeys: ["card.json", "spec.md", "plan-brief.md"],
    } as const;
    const decodedDetail = decodeDetail(detailWire);
    expect(encodeDetail(decodedDetail)).toEqual(detailWire);
  });

  it("validates getArtifact key traversal/safety guards and key/kind presence", () => {
    expect(Exit.isSuccess(decodeGetArtifact({ cardId: "card-1", key: "spec.md" }))).toBe(true);
    expect(
      Exit.isSuccess(decodeGetArtifact({ cardId: "card-1", key: "wireframes/index.html" })),
    ).toBe(true);
    expect(Exit.isSuccess(decodeGetArtifact({ cardId: "card-1", kind: "spec" }))).toBe(true);

    // Missing both key and kind
    expect(Exit.isFailure(decodeGetArtifact({ cardId: "card-1" }))).toBe(true);

    // Traversal, absolute, backslash, drive-colon, empty segments, NUL
    for (const badKey of [
      "",
      "   ",
      "../../.env",
      "wireframes/../../secret.html",
      "/etc/passwd",
      "wireframes\\index.html",
      "C:spec.md",
      "spec\0.md",
      ".",
      "..",
      "wireframes//index.html",
      "🚀".repeat(257),
    ]) {
      expect(Exit.isFailure(decodeGetArtifact({ cardId: "card-1", key: badKey }))).toBe(true);
    }
    expect(Exit.isSuccess(decodeGetArtifact({ cardId: "card-1", key: "🚀".repeat(200) }))).toBe(
      true,
    );
    expect(Exit.isSuccess(decodeGetArtifact({ cardId: "card-1", key: "🚀".repeat(256) }))).toBe(
      true,
    );

    for (const gitPath of [".git/config", ".GIT/HEAD", "sub/.git./foo", "GIT~1/config"]) {
      expect(Exit.isFailure(decodeSafeInputPath(gitPath))).toBe(true);
    }
    expect(Exit.isSuccess(decodeSafeInputPath("docs/spec.md"))).toBe(true);

    const artifactWire = {
      cardId: "card-1",
      key: "spec.md",
      kind: "spec",
      relativePath: ".legion/workflows/card-1/spec.md",
      digest: "sha256:deadbeef",
      content: "# Spec\n",
      revision: 2,
      updatedAt: "2026-11-24T00:30:00Z",
    } as const;
    expect(encodeArtifact(decodeArtifact(artifactWire))).toEqual(artifactWire);
  });

  it("validates expectedRevision >= 1, wireframesIndex clearing, and RPC request/error codecs", () => {
    for (const badRev of [0, -1, 1.5]) {
      expect(
        Exit.isFailure(decodeUpdateSpecUnknownExit({ cardId: "card-1", expectedRevision: badRev })),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeApproveSpecUnknownExit({ cardId: "card-1", expectedRevision: badRev }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeStartStageUnknownExit({
            cardId: "card-1",
            expectedRevision: badRev,
            stage: "plan",
          }),
        ),
      ).toBe(true);
    }

    // wireframesIndex: null clears; omitted leaves unchanged
    const cleared = decodeUpdateSpecSync({
      cardId: "card-1",
      expectedRevision: 1,
      wireframesIndex: null,
    });
    expect(cleared.wireframesIndex).toBeNull();

    const omitted = decodeUpdateSpecSync({
      cardId: "card-1",
      expectedRevision: 1,
    });
    expect(omitted.wireframesIndex).toBeUndefined();

    // subscribe & createCard & startStageResult JSON round-trips
    expect(decodeSubscribeSync({ projectId: "proj-1" }).projectId).toBe("proj-1");
    expect(
      decodeCreateCardSync({
        projectId: "proj-1",
        title: "  Card Title  ",
      }).title,
    ).toBe("Card Title");

    const startResultWire = {
      cardId: "card-1",
      revision: 3,
      stage: "plan",
      stageRunId: "sr-1",
      runId: "run-1",
      threadId: "thr-1",
      resumed: false,
    } as const;
    expect(encodeStartStageResult(decodeStartStageResult(startResultWire))).toEqual(
      startResultWire,
    );

    const errWire = {
      _tag: "WorkflowBoardRequestError",
      code: "revision_conflict",
      message: "Expected revision 1, current revision is 2",
      currentRevision: 2,
    } as const;
    const decodedErr = decodeErr(errWire);
    expect(decodedErr._tag).toBe("WorkflowBoardRequestError");
    expect(decodedErr.code).toBe("revision_conflict");
    expect(decodedErr.currentRevision).toBe(2);
    expect(encodeErr(decodedErr)).toEqual(errWire);

    const sampleCard = {
      cardId: "card-1",
      projectId: "proj-1",
      title: "T",
      stage: "spec",
      status: "active",
      revision: 2,
      currentTask: null,
      blockerSummary: null,
      nextAction: null,
      specApprovedRevision: null,
      specApprovalStatus: null,
      activeThreadId: null,
      createdAt: "2026-11-24T00:00:00Z",
      updatedAt: "2026-11-24T00:00:00Z",
      workspaceStrategy: { type: "root" },
      modelSelection: null,
      runtimeMode: null,
      interactionMode: null,
      spec: {
        title: "T",
        brief: "",
        sourceType: "idea",
        assistanceMode: "direct",
        acceptanceCriteria: [],
        constraints: [],
        sourceRefs: [],
        wireframesIndex: null,
        specDigest: null,
        updatedAt: "2026-11-24T00:00:00Z",
      },
      specApproval: null,
      stageRuns: [],
      revisions: [],
      artifactKeys: [],
    } as const;
    expect(
      Exit.isSuccess(
        decodeUpdateResultUnknownExit({
          cardId: "card-1",
          revision: 2,
          specDigest: null,
          specApprovalStatus: null,
          card: sampleCard,
        }),
      ),
    ).toBe(true);
    expect(
      decodeUpdateResultSync({
        cardId: "card-1",
        revision: 2,
        specDigest: "  sha256:abc  ",
        specApprovalStatus: null,
        card: sampleCard,
      }).specDigest,
    ).toBe("sha256:abc");
    for (const blankDigest of ["", "   "]) {
      expect(
        Exit.isFailure(
          decodeUpdateResultUnknownExit({
            cardId: "card-1",
            revision: 2,
            specDigest: blankDigest,
            specApprovalStatus: null,
            card: sampleCard,
          }),
        ),
      ).toBe(true);
    }
  });

  it("aligns clientRpcRequiredScopes, CLIENT_GUARDED_RPC_SCOPES, and WsRpcGroup with Rust permissions", () => {
    for (const readMethod of [
      "workflowBoard.subscribe",
      "workflowBoard.getCard",
      "workflowBoard.getArtifact",
    ]) {
      expect(clientRpcRequiredScopes(readMethod, {})).toEqual([AuthOrchestrationReadScope]);
      expect(Object.hasOwn(CLIENT_GUARDED_RPC_SCOPES, readMethod)).toBe(true);
      expect(WsRpcGroup.requests.has(readMethod)).toBe(true);
    }

    for (const writeMethod of [
      "workflowBoard.createCard",
      "workflowBoard.updateSpec",
      "workflowBoard.approveSpec",
      "workflowBoard.startStage",
    ]) {
      expect(clientRpcRequiredScopes(writeMethod, {})).toEqual([AuthOrchestrationOperateScope]);
      expect(Object.hasOwn(CLIENT_GUARDED_RPC_SCOPES, writeMethod)).toBe(true);
      expect(WsRpcGroup.requests.has(writeMethod)).toBe(true);
    }
  });
});

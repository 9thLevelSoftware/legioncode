import * as Exit from "effect/Exit";
import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import { AuthOrchestrationOperateScope } from "./auth.ts";
import { CLIENT_GUARDED_RPC_SCOPES, clientRpcRequiredScopes } from "./clientRpcPermissions.ts";
import { WS_METHODS, WsRpcGroup } from "./rpc.ts";
import {
  WorkflowBoardApplyCliImportInput,
  WorkflowBoardApplyCliImportResult,
  WorkflowBoardPreviewCliImportInput,
  WorkflowBoardPreviewCliImportResult,
  WorkflowBoardStartStageInput,
  WorkflowCardDetail,
} from "./workflowBoard.ts";
import { WorkflowBoardShipBlockerKind } from "./workflowBoardM3.ts";
import {
  WorkflowBoardCliImportHistoricalReceipt,
  WorkflowBoardCliImportLockEntry,
  WorkflowBoardCliImportMissingLogNote,
  WorkflowBoardCliImportProvenance,
  WorkflowBoardCliImportTaskPreview,
  WorkflowBoardCliImportUnsupportedPolicy,
} from "./workflowBoardM4.ts";

const decodePreviewInput = Schema.decodeUnknownExit(WorkflowBoardPreviewCliImportInput);
const decodeApplyInput = Schema.decodeUnknownExit(WorkflowBoardApplyCliImportInput);
const decodePreviewResult = Schema.decodeUnknownExit(WorkflowBoardPreviewCliImportResult);
const decodeApplyResult = Schema.decodeUnknownExit(WorkflowBoardApplyCliImportResult);
const decodeStartStageInput = Schema.decodeUnknownExit(WorkflowBoardStartStageInput);
const decodeCardDetail = Schema.decodeUnknownExit(WorkflowCardDetail);
const decodeBlockerKind = Schema.decodeUnknownExit(WorkflowBoardShipBlockerKind);
const decodeLockEntry = Schema.decodeUnknownExit(WorkflowBoardCliImportLockEntry);
const decodeUnsupportedPolicy = Schema.decodeUnknownExit(WorkflowBoardCliImportUnsupportedPolicy);
const decodeHistoricalReceipt = Schema.decodeUnknownExit(WorkflowBoardCliImportHistoricalReceipt);
const decodeMissingLogNote = Schema.decodeUnknownExit(WorkflowBoardCliImportMissingLogNote);
const decodeTaskPreview = Schema.decodeUnknownExit(WorkflowBoardCliImportTaskPreview);
const decodeProvenance = Schema.decodeUnknownExit(WorkflowBoardCliImportProvenance);

const sampleProvenance = {
  sourceSystem: "legion-cli" as const,
  sourceCliDir: ".legion-cli",
  sourceSpecId: "spec-t3-code",
  owner: "legioncode" as const,
  importedAt: "2026-11-26T00:00:00Z",
  lastSyncedAt: "2026-11-26T00:00:00Z",
  sourceDigest: "sha256:cli-source-1",
  decisionIds: ["D-001", "D-002"],
  assumptionIds: ["ASM-0001"],
  acceptanceCriterionIds: ["AC-P0-01"],
  historicalReceipts: [
    {
      id: "hist-apr-spec-card-1",
      kind: "spec_approval" as const,
      status: "historical" as const,
      sourcePath: ".legion-cli/workflow/spec-approval.yaml",
      recordedAt: "2026-10-22T01:00:17.476Z",
      recordedBy: "user",
      summary: "Historical spec approval from .legion-cli",
      requiresNativeRevalidation: true,
    },
  ],
  unsupportedPolicies: [
    {
      key: "sandbox.requireHardened",
      category: "sandbox",
      summary: "Hardened sandbox policy requires acknowledgement before Execute",
      configuredValue: "true",
      status: "unsupported" as const,
      blocksExecution: true,
      sourcePath: ".legion-cli/config.yaml",
    },
  ],
  acknowledgedPolicies: [],
  missingLogNotes: [
    {
      taskId: "TSK-0002",
      checkId: "chk-tsk-0002",
      relativePath: ".legion-cli/cache/runs/execute-mviwmzlq-24861406/verify-1.log",
      command: "node --test packages/provider-mimo/qualification/cli.test.mjs",
      note: "Gitignored cache log is absent on disk",
    },
  ],
  preservedArtifactPaths: [".legion-cli/specs/spec-t3-code/SPEC.md"],
  assurancePolicySummary: null,
};

const sampleCardDetailWire = {
  cardId: "cli-spec-t3-code",
  projectId: "proj-1",
  title: "T3 Code",
  stage: "execute" as const,
  status: "blocked" as const,
  revision: 1,
  currentTask: "TSK-0002",
  blockerSummary: "TSK-0002 is blocked",
  nextAction: "acknowledge_policies",
  specApprovedRevision: 1,
  specApprovalStatus: "historical" as const,
  activeThreadId: null,
  createdAt: "2026-11-26T00:00:00Z",
  updatedAt: "2026-11-26T00:00:00Z",
  planApprovedRevision: 1,
  planApprovalStatus: "historical" as const,
  workspaceStrategy: { type: "root" as const },
  modelSelection: null,
  runtimeMode: null,
  interactionMode: null,
  spec: {
    title: "T3 Code",
    brief: "Imported brief",
    sourceType: "codebase" as const,
    assistanceMode: "balanced" as const,
    acceptanceCriteria: [{ id: "AC-P0-01", text: "Provide reusable setup" }],
    constraints: ["Preserve existing providers"],
    sourceRefs: ["README.md"],
    wireframesIndex: null,
    specDigest: "sha256:spec-1",
    updatedAt: "2026-11-26T00:00:00Z",
  },
  specApproval: null,
  stageRuns: [],
  revisions: [],
  artifactKeys: ["card.json", "spec.md", "import-provenance.json"],
  cliImport: sampleProvenance,
};

describe("workflowBoard M4 CLI import contracts & permissions", () => {
  it("registers previewCliImport and applyCliImport in WS_METHODS, WsRpcGroup, and CLIENT_GUARDED_RPC_SCOPES", () => {
    expect(WS_METHODS.workflowBoardPreviewCliImport).toBe("workflowBoard.previewCliImport");
    expect(WS_METHODS.workflowBoardApplyCliImport).toBe("workflowBoard.applyCliImport");

    expect(WsRpcGroup.requests.has(WS_METHODS.workflowBoardPreviewCliImport)).toBe(true);
    expect(WsRpcGroup.requests.has(WS_METHODS.workflowBoardApplyCliImport)).toBe(true);

    expect(CLIENT_GUARDED_RPC_SCOPES[WS_METHODS.workflowBoardPreviewCliImport]).toBe(
      AuthOrchestrationOperateScope,
    );
    expect(CLIENT_GUARDED_RPC_SCOPES[WS_METHODS.workflowBoardApplyCliImport]).toBe(
      AuthOrchestrationOperateScope,
    );
    expect(clientRpcRequiredScopes(WS_METHODS.workflowBoardPreviewCliImport, {})).toEqual([
      AuthOrchestrationOperateScope,
    ]);
    expect(clientRpcRequiredScopes(WS_METHODS.workflowBoardApplyCliImport, {})).toEqual([
      AuthOrchestrationOperateScope,
    ]);
  });

  it("enforces path safety and non-nullable optional semantics on M4 RPC inputs", () => {
    expect(Exit.isSuccess(decodePreviewInput({ projectId: "proj-1" }))).toBe(true);
    expect(Exit.isSuccess(decodePreviewInput({ projectId: "proj-1", cliDir: ".legion-cli" }))).toBe(
      true,
    );
    expect(
      Exit.isFailure(decodePreviewInput({ projectId: "proj-1", cliDir: "../.legion-cli" })),
    ).toBe(true);
    expect(Exit.isFailure(decodePreviewInput({ projectId: "proj-1", cliDir: ".git" }))).toBe(true);
    expect(Exit.isFailure(decodePreviewInput({ projectId: "proj-1", cliDir: null }))).toBe(true);

    expect(
      Exit.isSuccess(
        decodeApplyInput({
          projectId: "proj-1",
          acknowledgeUnsupportedPolicies: true,
          acknowledgedPolicies: ["sandbox.requireHardened", "qa.passScore"],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(decodeApplyInput({ projectId: "proj-1", acknowledgedPolicies: null })),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeApplyInput({ projectId: "proj-1", acknowledgeUnsupportedPolicies: null }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodeStartStageInput({
          cardId: "cli-spec-t3-code",
          expectedRevision: 1,
          stage: "execute",
          acknowledgeUnsupportedPolicies: true,
          acknowledgedPolicies: ["sandbox.requireHardened"],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeStartStageInput({
          cardId: "cli-spec-t3-code",
          expectedRevision: 1,
          stage: "execute",
          acknowledgeUnsupportedPolicies: null,
        }),
      ),
    ).toBe(true);
  });

  it("decodes M4 preview/apply results, policy_unacknowledged blocker, and CardDetail.cliImport", () => {
    expect(Exit.isSuccess(decodeBlockerKind("policy_unacknowledged"))).toBe(true);
    expect(Exit.isSuccess(decodeProvenance(sampleProvenance))).toBe(true);
    expect(Exit.isSuccess(decodeCardDetail(sampleCardDetailWire))).toBe(true);
    expect(
      Exit.isSuccess(
        decodeLockEntry({
          kind: "engine_lock",
          relativePath: ".legion-cli/index/engine.lock",
          status: "active",
          pid: 1234,
          runId: null,
          startedAt: "2026-11-26T00:00:00Z",
          message: "Active CLI lock held",
        }),
      ),
    ).toBe(true);
    expect(Exit.isSuccess(decodeUnsupportedPolicy(sampleProvenance.unsupportedPolicies[0]))).toBe(
      true,
    );
    expect(Exit.isSuccess(decodeHistoricalReceipt(sampleProvenance.historicalReceipts[0]))).toBe(
      true,
    );
    expect(Exit.isSuccess(decodeMissingLogNote(sampleProvenance.missingLogNotes[0]))).toBe(true);
    expect(
      Exit.isSuccess(
        decodeTaskPreview({
          id: "TSK-0002",
          taskId: "TSK-0002",
          title: "Qualify MiMo",
          status: "blocked",
          blockerSummary: "Missing verify-1.log",
          verificationCommands: [
            ["node", "--test", "packages/provider-mimo/qualification/cli.test.mjs"],
          ],
          missingLogNotes: sampleProvenance.missingLogNotes,
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodePreviewResult({
          projectId: "proj-1",
          sourcePath: ".legion-cli",
          specId: "spec-t3-code",
          title: "T3 Code",
          stage: "execute",
          status: "blocked",
          existingCardId: null,
          alreadyImported: false,
          activeLocks: [],
          staleLocks: [],
          canApply: true,
          refusalReason: null,
          tasks: [],
          decisionIds: ["D-001"],
          assumptionIds: ["ASM-0001"],
          acceptanceCriterionIds: ["AC-P0-01"],
          historicalReceipts: sampleProvenance.historicalReceipts,
          unsupportedPolicies: sampleProvenance.unsupportedPolicies,
          missingLogNotes: sampleProvenance.missingLogNotes,
          warnings: [],
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodeApplyResult({
          cardId: "cli-spec-t3-code",
          projectId: "proj-1",
          revision: 1,
          created: true,
          sourceSpecId: "spec-t3-code",
          tasks: [],
          historicalReceipts: sampleProvenance.historicalReceipts,
          unsupportedPolicies: sampleProvenance.unsupportedPolicies,
          missingLogNotes: sampleProvenance.missingLogNotes,
          card: sampleCardDetailWire,
        }),
      ),
    ).toBe(true);
  });
});

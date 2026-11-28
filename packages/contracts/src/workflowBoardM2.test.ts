import * as Exit from "effect/Exit";
import * as Schema from "effect/Schema";
import { describe, expect, it } from "vite-plus/test";

import { AuthOrchestrationOperateScope } from "./auth.ts";
import { clientRpcRequiredScopes } from "./clientRpcPermissions.ts";
import { WS_METHODS } from "./rpc.ts";
import {
  WorkflowBoardApprovePlanInput,
  WorkflowBoardApproveSpecInput,
  WorkflowBoardChallengeStepInput,
  WorkflowBoardCreateCardInput,
  WorkflowBoardExploreStepInput,
  WorkflowBoardRequestErrorCode,
  WorkflowBoardReturnForRevisionInput,
  WorkflowBoardUpdatePlanInput,
  WorkflowBoardUpdateSpecInput,
  WorkflowBoardUpdateSpecResult,
  WorkflowCardDetail,
} from "./workflowBoard.ts";
import { WorkflowBoardArtifactKey, WorkflowBoardTaskContract } from "./workflowBoardPlanning.ts";

const decodeCreateCard = Schema.decodeUnknownExit(WorkflowBoardCreateCardInput);
const decodeUpdateSpec = Schema.decodeUnknownExit(WorkflowBoardUpdateSpecInput);
const decodeArtifactKey = Schema.decodeUnknownExit(WorkflowBoardArtifactKey);
const decodeTaskContract = Schema.decodeUnknownExit(WorkflowBoardTaskContract);
const decodeApproveSpec = Schema.decodeUnknownExit(WorkflowBoardApproveSpecInput);
const decodeExploreStep = Schema.decodeUnknownExit(WorkflowBoardExploreStepInput);
const decodeChallengeStep = Schema.decodeUnknownExit(WorkflowBoardChallengeStepInput);
const decodeUpdatePlan = Schema.decodeUnknownExit(WorkflowBoardUpdatePlanInput);
const decodeApprovePlan = Schema.decodeUnknownExit(WorkflowBoardApprovePlanInput);
const decodeReturnForRevision = Schema.decodeUnknownExit(WorkflowBoardReturnForRevisionInput);
const decodeErrorCode = Schema.decodeUnknownExit(WorkflowBoardRequestErrorCode);

const cardDetailCodec = Schema.toCodecJson(WorkflowCardDetail);
const decodeCardDetail = Schema.decodeSync(cardDetailCodec);
const encodeCardDetail = Schema.encodeSync(cardDetailCodec);

const updateSpecResultCodec = Schema.toCodecJson(WorkflowBoardUpdateSpecResult);
const decodeUpdateSpecResult = Schema.decodeSync(updateSpecResultCodec);
const encodeUpdateSpecResult = Schema.encodeSync(updateSpecResultCodec);

const baseDetailWire = {
  cardId: "card-m2",
  projectId: "proj-1",
  title: "M2 Planning Feature",
  stage: "plan",
  status: "active",
  revision: 4,
  currentTask: null,
  blockerSummary: null,
  nextAction: "approve_plan",
  specApprovedRevision: 3,
  specApprovalStatus: "valid",
  activeThreadId: null,
  createdAt: "2026-11-24T00:00:00Z",
  updatedAt: "2026-11-24T01:00:00Z",
  workspaceStrategy: { type: "root" },
  modelSelection: null,
  runtimeMode: null,
  interactionMode: null,
  spec: {
    title: "M2 Planning Feature",
    brief: "Deliver M2 contracts",
    sourceType: "idea",
    assistanceMode: "guided",
    acceptanceCriteria: [{ id: "AC-01", text: "Persists M2 planning state" }],
    constraints: ["No auto-commits"],
    sourceRefs: ["README.md"],
    wireframesIndex: null,
    specDigest: "sha256:spec-1",
    updatedAt: "2026-11-24T00:30:00Z",
    intakeGoal: "change",
  },
  specApproval: null,
  stageRuns: [],
  revisions: [],
  artifactKeys: ["card.json", "spec.md", "plan.md"],
  wireframesIndex: null,
  planApproval: null,
  planApprovalStatus: null,
  lenses: [
    { lens: "ux", applicable: false, reason: "Backend-only contract milestone." },
    { lens: "functional", applicable: true, summary: "Typed schemas and CAS persistence." },
  ],
  alternatives: [
    {
      id: "alt-a",
      title: "Normalized SQLite tables",
      summary: "Seven child tables with FK cascades.",
      tradeoffs: "Explicit schema migrations.",
    },
    {
      id: "alt-b",
      title: "Single JSON blob",
      summary: "Store plan in one JSON column.",
      tradeoffs: "Harder per-row status updates.",
    },
  ],
  selectedAlternativeId: "alt-a",
  selectionRationale: "Normalized tables support M3 check/task tracking.",
  strategy: "outcomes",
  granularity: "balanced",
  tasks: [
    {
      taskId: "TSK-0001",
      title: "Write contracts",
      dependsOn: [],
      filesAllowed: ["packages/contracts/src/workflowBoard.ts"],
      filesForbidden: [".env"],
      testingMethod: "test-first",
    },
  ],
  acMappings: [
    {
      criterionId: "AC-01",
      method: "automated",
      command: ["vp", "test", "run", "packages/contracts/src/workflowBoardM2.test.ts"],
    },
  ],
  history: [],
} as const;

describe("workflowBoard M2 contracts and validators", () => {
  it("validates createCard brief non-whitespace, credential inputPaths, and approveSpec expectedDigest", () => {
    expect(
      Exit.isSuccess(
        decodeCreateCard({
          projectId: "proj-1",
          title: "Card",
          brief: "Valid brief",
          inputPaths: ["README.md", "docs/spec.md"],
          intakeGoal: "change",
          wireframesEnabled: false,
        }),
      ),
    ).toBe(true);

    // Omitted brief is allowed, whitespace-only brief is rejected
    expect(Exit.isSuccess(decodeCreateCard({ projectId: "proj-1", title: "Card" }))).toBe(true);
    expect(
      Exit.isFailure(decodeCreateCard({ projectId: "proj-1", title: "Card", brief: "   " })),
    ).toBe(true);

    for (const badPath of [
      ".env",
      ".env.local",
      "config/.env.production",
      "certs/server.pem",
      "keys/id_rsa",
      "keys/id_ed25519",
      ".ssh/config",
      ".npmrc",
      ".netrc",
      "tls/site.key",
      "tls/ca.crt",
      "../../etc/passwd",
    ]) {
      expect(
        Exit.isFailure(
          decodeCreateCard({ projectId: "proj-1", title: "Card", inputPaths: [badPath] }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeCreateCard({ projectId: "proj-1", title: "Card", briefPath: badPath }),
        ),
      ).toBe(true);
    }
    expect(
      Exit.isSuccess(
        decodeCreateCard({ projectId: "proj-1", title: "Card", briefPath: "docs/brief.md" }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodeApproveSpec({ cardId: "card-1", expectedRevision: 2, expectedDigest: "sha256:abc" }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeApproveSpec({ cardId: "card-1", expectedRevision: 2, expectedDigest: "   " }),
      ),
    ).toBe(true);
  });

  it("enforces exploreStep <=3 cap, action/choice aliases, and edit answer requirement", () => {
    expect(
      Exit.isSuccess(
        decodeExploreStep({
          cardId: "card-1",
          expectedRevision: 1,
          answers: [
            { decisionId: "D1", action: "edit", answer: "Keep SQLite" },
            { decisionId: "D2", action: "pause" },
            { decisionId: "D3", choice: "recommend", note: "Accepted" },
          ],
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodeExploreStep({
          cardId: "card-1",
          expectedRevision: 1,
          action: "resume",
        }),
      ),
    ).toBe(true);

    // > 3 answers rejected
    expect(
      Exit.isFailure(
        decodeExploreStep({
          cardId: "card-1",
          expectedRevision: 1,
          answers: [
            { decisionId: "D1", action: "explain" },
            { decisionId: "D2", action: "explain" },
            { decisionId: "D3", action: "explain" },
            { decisionId: "D4", action: "explain" },
          ],
        }),
      ),
    ).toBe(true);

    // edit without answer/note rejected
    expect(
      Exit.isFailure(
        decodeExploreStep({
          cardId: "card-1",
          expectedRevision: 1,
          answers: [{ decisionId: "D1", action: "edit" }],
        }),
      ),
    ).toBe(true);
  });

  it("enforces challengeStep <=3 concerns, dismiss reason, synthesis immutability, no approved bypass, and manualFallback", () => {
    expect(
      Exit.isSuccess(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "synthesize",
          concerns: [
            {
              id: "C1",
              summary: "Migration safety",
              disposition: "answer",
              answer: "Idempotent DDL",
            },
          ],
        }),
      ),
    ).toBe(true);

    // approved: true bypass rejected
    expect(
      Exit.isFailure(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "synthesize",
          approved: true,
        }),
      ),
    ).toBe(true);

    // mutatedAnswer != answer rejected
    expect(
      Exit.isFailure(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "synthesize",
          concerns: [
            {
              id: "C1",
              summary: "Guard",
              disposition: "answer",
              answer: "Original",
              mutatedAnswer: "Mutated",
            },
          ],
        }),
      ),
    ).toBe(true);

    // dismiss without reason rejected
    expect(
      Exit.isFailure(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "resolve",
          concerns: [{ id: "C1", summary: "Guard", disposition: "dismiss", reason: "   " }],
        }),
      ),
    ).toBe(true);

    // manualFallback exact "I acknowledge", empty unresolvedConcernIds, and 3 answers
    expect(
      Exit.isSuccess(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "manualFallback",
          acknowledgementToken: "I acknowledge",
          unresolvedConcernIds: [],
          answers: ["Measurable outcome", "Rollback on failure", "Backward compatible"],
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isFailure(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "manualFallback",
          acknowledgementToken: "i acknowledge",
          unresolvedConcernIds: [],
          answers: ["a", "b", "c"],
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isFailure(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "manualFallback",
          acknowledgementToken: "I acknowledge",
          unresolvedConcernIds: ["C1"],
          answers: ["a", "b", "c"],
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isFailure(
        decodeChallengeStep({
          cardId: "card-1",
          expectedRevision: 2,
          action: "manualFallback",
          acknowledgementToken: "I acknowledge",
          unresolvedConcernIds: [],
          answers: ["a", "b"],
        }),
      ),
    ).toBe(true);
  });

  it("enforces updatePlan lenses, 2 distinct alternatives, strategy invariants, disjoint files, acyclic DAG, and argv-only commands", () => {
    const validAltA = {
      id: "alt-a",
      title: "Option A",
      summary: "Normalized schema",
      tradeoffs: "More tables",
    };
    const validAltB = {
      id: "alt-b",
      title: "Option B",
      summary: "Document store",
      tradeoffs: "Less relational integrity",
    };

    expect(
      Exit.isSuccess(
        decodeUpdatePlan({
          cardId: "card-1",
          expectedRevision: 3,
          lenses: [
            { lens: "ux", applicable: false, reason: "No UI" },
            { lens: "functional", applicable: true, summary: "Core RPCs" },
          ],
          alternatives: [validAltA, validAltB],
          selectedAlternativeId: "alt-a",
          selectionRationale: "Stronger FK guarantees",
          strategy: "risk-first",
          strategyMetadata: { uncertainty: "WAL contention", probeTaskId: "TSK-0001" },
          granularity: "balanced",
          tasks: [
            {
              taskId: "TSK-0001",
              title: "Probe WAL",
              dependsOn: [],
              filesAllowed: ["src/a.ts"],
              filesForbidden: ["src/b.ts"],
              testingMethod: "test-first",
            },
            {
              taskId: "TSK-0002",
              title: "Implement",
              dependsOn: ["TSK-0001"],
              filesAllowed: ["src/b.ts"],
              filesForbidden: ["src/a.ts"],
              testingMethod: "regression-first",
            },
          ],
          acMappings: [
            {
              criterionId: "AC-01",
              method: "automated",
              command: ["cargo", "test", "-p", "legion-persistence"],
            },
            {
              criterionId: "AC-02",
              method: "manual",
              procedure: "Verify visual badge in board overview.",
            },
          ],
        }),
      ),
    ).toBe(true);

    // Reject 1 alternative or identical alternatives or unknown selectedAlternativeId
    expect(
      Exit.isFailure(
        decodeUpdatePlan({ cardId: "card-1", expectedRevision: 3, alternatives: [validAltA] }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeUpdatePlan({
          cardId: "card-1",
          expectedRevision: 3,
          alternatives: [validAltA, { ...validAltA, id: "alt-dup" }],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeUpdatePlan({
          cardId: "card-1",
          expectedRevision: 3,
          alternatives: [validAltA, validAltB],
          selectedAlternativeId: "alt-999",
          selectionRationale: "Missing option",
        }),
      ),
    ).toBe(true);

    // Reject overlapping filesAllowed across tasks or cyclic DAG
    expect(
      Exit.isFailure(
        decodeUpdatePlan({
          cardId: "card-1",
          expectedRevision: 3,
          tasks: [
            {
              taskId: "TSK-1",
              title: "A",
              filesAllowed: ["src/shared.ts"],
              testingMethod: "test-first",
            },
            {
              taskId: "TSK-2",
              title: "B",
              filesAllowed: ["src/shared.ts"],
              testingMethod: "test-first",
            },
          ],
        }),
      ),
    ).toBe(true);
    for (const edgeField of ["dependsOn", "blockedBy", "blocks"] as const) {
      for (const selfRef of ["T1", " T1 "]) {
        expect(
          Exit.isFailure(
            decodeTaskContract({
              taskId: "T1",
              title: "Self-reference check",
              [edgeField]: [selfRef],
              filesAllowed: ["src/a.ts"],
              testingMethod: "test-first",
            }),
          ),
        ).toBe(true);
      }
    }
    expect(
      Exit.isSuccess(
        decodeTaskContract({
          taskId: "T1",
          title: "Valid standalone task",
          dependsOn: ["T0"],
          blockedBy: ["T0"],
          blocks: ["T2"],
          filesAllowed: ["src/a.ts"],
          testingMethod: "test-first",
        }),
      ),
    ).toBe(true);

    for (const edgeField of ["dependsOn", "blocks"] as const) {
      expect(
        Exit.isFailure(
          decodeUpdatePlan({
            cardId: "card-1",
            expectedRevision: 3,
            tasks: [
              {
                taskId: "TSK-1",
                title: "A",
                [edgeField]: ["TSK-2"],
                filesAllowed: ["src/a.ts"],
                testingMethod: "test-first",
              },
              {
                taskId: "TSK-2",
                title: "B",
                [edgeField]: ["TSK-1"],
                filesAllowed: ["src/b.ts"],
                testingMethod: "test-first",
              },
            ],
          }),
        ),
      ).toBe(true);
    }

    // Reject shell wrapper commands in acMappings
    expect(
      Exit.isFailure(
        decodeUpdatePlan({
          cardId: "card-1",
          expectedRevision: 3,
          acMappings: [
            { criterionId: "AC-01", method: "automated", command: ["sh", "-c", "a && b"] },
          ],
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeUpdatePlan({
          cardId: "card-1",
          expectedRevision: 3,
          acMappings: [
            { criterionId: "AC-01", method: "automated", command: ["cmd.exe", "/c", "dir"] },
          ],
        }),
      ),
    ).toBe(true);

    // Reject env -S / --split-string, argument-taking flags, and chained env shell-wrapper bypasses
    for (const badEnv of [
      ["env", "-S", "sh -c id"],
      ["/usr/bin/env", "-iS", "sh -c id"],
      ["env", "--split-string=sh -c id"],
      ["env", "--split-string", "sh -c id"],
      ["env", "-u", "PATH", "sh", "-c", "id"],
      ["env", "-C", "/tmp", "bash", "-lc", "id"],
      ["env", "--unset", "FOO", "pwsh", "-Command", "id"],
      ["env", "env", "sh", "-c", "id"],
      ["env", "-u"],
    ]) {
      expect(
        Exit.isFailure(
          decodeUpdatePlan({
            cardId: "card-1",
            expectedRevision: 3,
            acMappings: [{ criterionId: "AC-01", method: "automated", command: badEnv }],
          }),
        ),
      ).toBe(true);
    }
    for (const goodEnv of [
      ["env", "FOO=1", "cargo", "test"],
      ["env", "--", "cargo", "test"],
      ["/usr/bin/env", "-i", "PATH=/usr/bin", "vp", "test", "run"],
    ]) {
      expect(
        Exit.isSuccess(
          decodeUpdatePlan({
            cardId: "card-1",
            expectedRevision: 3,
            acMappings: [{ criterionId: "AC-01", method: "automated", command: goodEnv }],
          }),
        ),
      ).toBe(true);
    }

    // Reject blank strings across all 10 M2 string-array fields
    for (const blank of ["", "   "]) {
      for (const field of ["dependsOn", "blockedBy", "blocks"] as const) {
        expect(
          Exit.isFailure(
            decodeUpdatePlan({
              cardId: "card-1",
              expectedRevision: 3,
              tasks: [
                {
                  taskId: "TSK-1",
                  title: "A",
                  [field]: [blank],
                  filesAllowed: ["src/a.ts"],
                  testingMethod: "test-first",
                },
              ],
            }),
          ),
        ).toBe(true);
      }
      expect(
        Exit.isFailure(
          decodeUpdatePlan({
            cardId: "card-1",
            expectedRevision: 3,
            acMappings: [{ criterionId: "AC-01", method: "automated", checkIds: [blank] }],
          }),
        ),
      ).toBe(true);
      for (const field of [
        "dependentTaskIds",
        "compatibilityTaskIds",
        "transitionTaskIds",
        "retirementTaskIds",
      ] as const) {
        expect(
          Exit.isFailure(
            decodeUpdatePlan({
              cardId: "card-1",
              expectedRevision: 3,
              strategy: "outcomes",
              strategyMetadata: { [field]: [blank] },
            }),
          ),
        ).toBe(true);
      }
      expect(
        Exit.isFailure(
          decodeChallengeStep({
            cardId: "card-1",
            expectedRevision: 2,
            action: "resolve",
            unresolvedConcernIds: [blank],
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeChallengeStep({
            cardId: "card-1",
            expectedRevision: 2,
            action: "resolve",
            answers: [blank],
          }),
        ),
      ).toBe(true);
    }
  });

  it("validates approvePlan, returnForRevision, wireframesIndex: null round-trip, and M2 permissions", () => {
    expect(
      Exit.isSuccess(
        decodeApprovePlan({
          cardId: "card-1",
          expectedRevision: 4,
          expectedSpecDigest: "sha256:spec",
          expectedPlanDigest: "sha256:plan",
        }),
      ),
    ).toBe(true);

    expect(
      Exit.isSuccess(
        decodeReturnForRevision({
          cardId: "card-1",
          expectedRevision: 4,
          targetStage: "spec",
          reason: "Clarify AC-02",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeReturnForRevision({
          cardId: "card-1",
          expectedRevision: 4,
          targetStage: "ship",
          reason: "Invalid forward target",
        }),
      ),
    ).toBe(true);
    expect(
      Exit.isFailure(
        decodeReturnForRevision({
          cardId: "card-1",
          expectedRevision: 4,
          targetStage: "spec",
          reason: "   ",
        }),
      ),
    ).toBe(true);

    const decodedDetail = decodeCardDetail(baseDetailWire);
    expect(encodeCardDetail(decodedDetail)).toMatchObject({ wireframesIndex: null });

    const decodeDetailExit = Schema.decodeUnknownExit(WorkflowCardDetail);
    for (const field of ["wireframesDigest", "selectedAlternativeId"] as const) {
      for (const blank of ["", "   "]) {
        expect(Exit.isFailure(decodeDetailExit({ ...baseDetailWire, [field]: blank }))).toBe(true);
      }
      expect(Exit.isSuccess(decodeDetailExit({ ...baseDetailWire, [field]: null }))).toBe(true);
    }

    const updateSpecResWire = {
      cardId: "card-m2",
      revision: 4,
      specDigest: "sha256:spec-1",
      specApprovalStatus: "stale",
      card: baseDetailWire,
      wireframesIndex: null,
      specApproval: null,
      planApproval: null,
    } as const;
    expect(encodeUpdateSpecResult(decodeUpdateSpecResult(updateSpecResWire))).toMatchObject({
      wireframesIndex: null,
    });

    for (const code of [
      "plan_not_approved",
      "exploration_unresolved",
      "challenge_unresolved",
      "external_edit_detected",
      "contract_conflict",
      "digest_mismatch",
    ]) {
      expect(Exit.isSuccess(decodeErrorCode(code))).toBe(true);
    }

    for (const method of [
      WS_METHODS.workflowBoardExploreStep,
      WS_METHODS.workflowBoardChallengeStep,
      WS_METHODS.workflowBoardUpdatePlan,
      WS_METHODS.workflowBoardApprovePlan,
      WS_METHODS.workflowBoardReturnForRevision,
    ]) {
      expect(clientRpcRequiredScopes(method, {})).toEqual([AuthOrchestrationOperateScope]);
    }
  });

  it("trims artifact keys and rejects .git/.GIT/GIT~1 in wireframesIndex, filesAllowed, and filesForbidden", () => {
    expect(Exit.isFailure(decodeArtifactKey(" .."))).toBe(true);
    expect(Exit.isFailure(decodeArtifactKey(" ."))).toBe(true);
    expect(Exit.isSuccess(decodeArtifactKey("  wireframes/index.html  "))).toBe(true);

    for (const gitPath of [
      ".git/index.html",
      ".GIT/HEAD",
      "sub/.git./index.html",
      "GIT~1/config",
      "  .git/hooks  ",
    ]) {
      expect(
        Exit.isFailure(
          decodeCreateCard({
            projectId: "proj-1",
            title: "Git wireframe",
            wireframesIndex: gitPath,
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeUpdateSpec({
            cardId: "card-1",
            expectedRevision: 1,
            wireframesIndex: gitPath,
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeTaskContract({
            taskId: "TSK-1",
            title: "Git allowed",
            filesAllowed: [gitPath],
            testingMethod: "test-first",
          }),
        ),
      ).toBe(true);
      expect(
        Exit.isFailure(
          decodeTaskContract({
            taskId: "TSK-1",
            title: "Git forbidden",
            filesAllowed: ["src/a.ts"],
            filesForbidden: [gitPath],
            testingMethod: "test-first",
          }),
        ),
      ).toBe(true);
    }
  });
});

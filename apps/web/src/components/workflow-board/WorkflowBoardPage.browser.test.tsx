import { AsyncResult, Atom } from "effect/reactivity";
import * as Cause from "effect/Cause";
import {
  DEFAULT_SERVER_SETTINGS,
  EnvironmentId,
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProjectId,
  ProviderInstanceId,
  RunId,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WorkflowBoardNonGitArtifactKey,
  WorkflowBoardStageRunId,
  WorkflowCardId,
  type GitRunStackedActionResult,
  type ProjectWorkflowBoardSnapshot,
  type ServerProvider,
  type VcsStatusResult,
  type WorkflowBoardApplyCliImportResult,
  type WorkflowBoardArtifact,
  type WorkflowBoardCardSummary,
  type WorkflowBoardGetCardDiffResult,
  type WorkflowBoardPreviewCliImportResult,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { describe, expect, it, vi } from "vite-plus/test";
import { page as browserPage, userEvent } from "vite-plus/test/browser";

import "~/index.css";
import { render } from "../../../test/render";
import { useStableCallback } from "../../hooks/useStableCallback";
import type { SidebarProjectSnapshot } from "../../sidebarProjectGrouping";
import { SidebarProjectFolderHeader } from "../sidebar/SidebarProjectFolderHeader";
import { WorkflowBoardPage } from "./WorkflowBoardPage";
import type { WorkflowBoardSearch } from "./workflowBoard.logic";

const ENV_ID = EnvironmentId.make("env-1");
const ENV_2_ID = EnvironmentId.make("env-2");
const PROJECT_ID = ProjectId.make("proj-1");
const PROJECT_2_ID = ProjectId.make("proj-2");
const CARD_1 = WorkflowCardId.make("card-1");
const CARD_2 = WorkflowCardId.make("card-2");
const THREAD_ID = ThreadId.make("thread-plan-1");

const DEV_REPLAY_PROVIDER = {
  instanceId: ProviderInstanceId.make("devReplay"),
  driver: "devReplay",
  enabled: true,
  installed: true,
  status: "ready",
  availability: "available",
  auth: { status: "authenticated" },
  models: [{ slug: "dev-replay", name: "Replay (dev)", isDefault: true, isCustom: false }],
} as unknown as ServerProvider;

const PROJECT_GROUP = {
  id: PROJECT_ID,
  environmentId: ENV_ID,
  name: "t3code",
  workspaceRoot: "/repo/t3code",
  repositoryIdentity: null,
  defaultModelSelection: null,
  scripts: [],
  trackedFolders: [],
  createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
  updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
  isScratch: false,
  projectKey: "repo:t3code",
  displayName: "t3code",
  groupedProjectCount: 1,
  environmentPresence: "local-only",
  allRemoteMembersAreDesktopLocal: false,
  allRemoteMembersAreWsl: false,
  remoteEnvironmentLabels: [],
  pinned: false,
  archived: false,
  memberProjectRefs: [{ environmentId: ENV_ID, projectId: PROJECT_ID }],
  memberProjects: [
    {
      id: PROJECT_ID,
      environmentId: ENV_ID,
      name: "t3code",
      workspaceRoot: "/repo/t3code",
      repositoryIdentity: null,
      defaultModelSelection: null,
      scripts: [],
      trackedFolders: [],
      createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      isScratch: false,
      physicalProjectKey: "env-1:/repo/t3code",
      environmentLabel: "Local",
    },
  ],
} as unknown as SidebarProjectSnapshot;

const MULTI_CHECKOUT_GROUP = {
  ...PROJECT_GROUP,
  groupedProjectCount: 2,
  environmentPresence: "mixed",
  remoteEnvironmentLabels: ["Remote"],
  memberProjectRefs: [
    { environmentId: ENV_ID, projectId: PROJECT_ID },
    { environmentId: ENV_2_ID, projectId: PROJECT_2_ID },
  ],
  memberProjects: [
    ...PROJECT_GROUP.memberProjects,
    {
      id: PROJECT_2_ID,
      environmentId: ENV_2_ID,
      name: "t3code",
      workspaceRoot: "/srv/t3code",
      repositoryIdentity: null,
      defaultModelSelection: null,
      scripts: [],
      trackedFolders: [],
      createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      isScratch: false,
      physicalProjectKey: "env-2:/srv/t3code",
      environmentLabel: "Remote",
    },
  ],
} as unknown as SidebarProjectSnapshot;

const state = vi.hoisted(() => ({
  search: {} as WorkflowBoardSearch,
  connected: true,
  multiCheckout: false,
  workflowsEnabled: true,
  projectWorkflowBoard: true as boolean | undefined,
  canRead: true,
  canOperate: true,
  snapshot: null as ProjectWorkflowBoardSnapshot | null,
  cardDetail: null as WorkflowCardDetail | null,
  artifact: null as WorkflowBoardArtifact | null,
  cardDiff: null as WorkflowBoardGetCardDiffResult | null,
  vcsStatus: null as VcsStatusResult | null,
  approveDeliveryError: null as Error | null,
  previewCliImportResult: null as WorkflowBoardPreviewCliImportResult | null,
  previewCliImportError: null as Error | null,
  previewCliImportHandler: null as
    | ((input: {
        readonly projectId: ProjectId;
        readonly cliDir?: string;
      }) => Promise<
        | { readonly _tag: "Success"; readonly value: WorkflowBoardPreviewCliImportResult }
        | { readonly _tag: "Failure"; readonly cause: Cause.Cause<unknown> }
      >)
    | null,
  applyCliImportResult: null as WorkflowBoardApplyCliImportResult | null,
  applyCliImportError: null as Error | null,
  applyCliImportHandler: null as
    | ((
        input: unknown,
      ) => Promise<
        | { readonly _tag: "Success"; readonly value: WorkflowBoardApplyCliImportResult }
        | { readonly _tag: "Failure"; readonly cause: Cause.Cause<unknown> }
      >)
    | null,
  proposeAmendmentHandler: null as
    | ((
        input: unknown,
      ) => Promise<
        | { readonly _tag: "Success"; readonly value: unknown }
        | { readonly _tag: "Failure"; readonly cause: Cause.Cause<unknown> }
      >)
    | null,
  providers: [] as ServerProvider[],
  threadShellPresent: true,
  requestedQueryKinds: new Set<string>(),
  calls: [] as Array<readonly [action: string, input: unknown]>,
  navigations: [] as Array<unknown>,
}));

const canReadAtom = Atom.make(true);
const cannotReadAtom = Atom.make(false);
const canOperateAtom = Atom.make(true);
const cannotOperateAtom = Atom.make(false);
const emptyServerConfigAtom = Atom.make(null);
const noopVcsActionStateAtom = Atom.make({
  operation: null,
  isRunning: false,
  currentLabel: null,
  hookName: null,
  lastOutputLine: null,
  phaseStartedAtMs: null,
  hookStartedAtMs: null,
});
const noopRevalidateAtom = Atom.make(null);

function makeSummary(
  overrides: Partial<WorkflowBoardCardSummary> & Pick<WorkflowBoardCardSummary, "cardId" | "title">,
): WorkflowBoardCardSummary {
  return {
    projectId: PROJECT_ID,
    stage: "spec",
    status: "active",
    revision: PositiveInt.make(1),
    currentTask: null,
    blockerSummary: null,
    nextAction: "draft_spec",
    specApprovedRevision: null,
    specApprovalStatus: null,
    activeThreadId: null,
    createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
    updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
    ...overrides,
  };
}

function makeDetail(overrides: Partial<WorkflowCardDetail> = {}): WorkflowCardDetail {
  return {
    ...makeSummary({ cardId: CARD_1, title: "Feature A" }),
    workspaceStrategy: { type: "root" },
    modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
    runtimeMode: "full-access",
    interactionMode: "default",
    spec: {
      title: "Feature A",
      brief: "Initial idea",
      sourceType: "idea",
      assistanceMode: "balanced",
      acceptanceCriteria: [{ id: "AC-1", text: "Board renders" }],
      constraints: [],
      sourceRefs: [],
      wireframesIndex: null,
      specDigest: "sha256:digest-1",
      updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
    },
    specApproval: null,
    stageRuns: [],
    revisions: [],
    artifactKeys: [WorkflowBoardArtifactKey.make("spec.md")],
    ...overrides,
  };
}

function setup(overrides: Partial<typeof state> = {}) {
  state.search = {};
  state.connected = true;
  state.multiCheckout = false;
  state.workflowsEnabled = true;
  state.projectWorkflowBoard = true;
  state.canRead = true;
  state.canOperate = true;
  state.snapshot = {
    projectId: PROJECT_ID,
    revision: NonNegativeInt.make(1),
    cards: [],
    generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
  };
  state.cardDetail = null;
  state.artifact = null;
  state.cardDiff = null;
  state.vcsStatus = null;
  state.approveDeliveryError = null;
  state.previewCliImportResult = null;
  state.previewCliImportError = null;
  state.previewCliImportHandler = null;
  state.applyCliImportResult = null;
  state.applyCliImportError = null;
  state.applyCliImportHandler = null;
  state.proposeAmendmentHandler = null;
  state.providers = [DEV_REPLAY_PROVIDER];
  state.threadShellPresent = true;
  state.requestedQueryKinds = new Set<string>();
  state.calls = [];
  state.navigations = [];
  Object.assign(state, overrides);
}

vi.mock("@tanstack/react-router", async (original) => {
  const actual = await original<typeof import("@tanstack/react-router")>();
  return {
    ...actual,
    useParams: () => ({ projectKey: "repo:t3code" }),
    useSearch: () => state.search,
    useNavigate: () => (opts: unknown) => {
      state.navigations.push(opts);
      const nextSearch = (opts as { readonly search?: WorkflowBoardSearch }).search;
      if (nextSearch !== undefined) {
        state.search = nextSearch;
      }
      return Promise.resolve();
    },
    Link: ({
      to,
      children,
      className,
    }: {
      readonly to: string;
      readonly children?: React.ReactNode;
      readonly className?: string;
    }) => (
      <a href={to} className={className}>
        {children}
      </a>
    ),
  };
});

vi.mock("../../state/workflowBoard", () => {
  const readPerm = () => (state.canRead ? canReadAtom : cannotReadAtom);
  const operatePerm = () => (state.canOperate ? canOperateAtom : cannotOperateAtom);
  return {
    workflowBoard: {
      readPermissionAtom: readPerm,
      boardSnapshot: () => ({ kind: "snapshot" }),
      cardDetail: () => ({ kind: "cardDetail" }),
      artifact: () => ({ kind: "artifact" }),
      cardDiff: () => ({ kind: "cardDiff" }),
      invalidateCard: vi.fn(),
      previewCliImport: { permissionAtom: operatePerm },
      applyCliImport: { permissionAtom: operatePerm },
      createCard: { permissionAtom: operatePerm },
      updateSpec: { permissionAtom: operatePerm },
      approveSpec: { permissionAtom: operatePerm },
      exploreStep: { permissionAtom: operatePerm },
      challengeStep: { permissionAtom: operatePerm },
      updatePlan: { permissionAtom: operatePerm },
      approvePlan: { permissionAtom: operatePerm },
      returnForRevision: { permissionAtom: operatePerm },
      startStage: { permissionAtom: operatePerm },
      pauseCard: { permissionAtom: operatePerm },
      resumeCard: { permissionAtom: operatePerm },
      retryStep: { permissionAtom: operatePerm },
      proposeAmendment: { permissionAtom: operatePerm },
      resolveFinding: { permissionAtom: operatePerm },
      recordManualAcceptance: { permissionAtom: operatePerm },
      approveDelivery: { permissionAtom: operatePerm },
    },
  };
});

vi.mock(import("../../state/query"), async (original) => ({
  ...(await original()),
  useEnvironmentQuery: ((request: { readonly kind?: string } | null) => {
    if (request === null) {
      return {
        data: null,
        dataUpdatedAt: 0,
        error: null,
        failure: null,
        isPending: false,
        isSuccess: false,
        refresh: vi.fn(),
      };
    }
    if (request.kind !== undefined) {
      state.requestedQueryKinds.add(request.kind);
    }
    const data =
      request.kind === "snapshot"
        ? state.snapshot
        : request.kind === "cardDetail"
          ? state.cardDetail
          : request.kind === "artifact"
            ? state.artifact
            : request.kind === "cardDiff"
              ? state.cardDiff
              : request.kind === "vcsStatus"
                ? state.vcsStatus
                : null;
    return {
      data,
      dataUpdatedAt: 1,
      error: null,
      failure: null,
      isPending: false,
      isSuccess: data !== null,
      refresh: vi.fn(),
    };
  }) as never,
}));

vi.mock(import("../../state/use-atom-command"), async (original) => ({
  ...(await original()),
  useAtomCommand: ((_command: unknown, options: { readonly label: string }) =>
    useStableCallback((payload: { readonly input: unknown }) => {
      state.calls.push([options.label, payload.input]);
      if (options.label === "workflow board preview cli import") {
        if (state.previewCliImportHandler !== null) {
          return state.previewCliImportHandler(
            payload.input as { readonly projectId: ProjectId; readonly cliDir?: string },
          );
        }
        if (state.previewCliImportError !== null) {
          return Promise.resolve({
            _tag: "Failure",
            cause: Cause.fail(state.previewCliImportError),
          });
        }
        return Promise.resolve({
          _tag: "Success",
          value:
            state.previewCliImportResult ??
            ({
              projectId: PROJECT_ID,
              sourcePath: WorkflowBoardArtifactKey.make(".legion-cli"),
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
              decisionIds: [],
              assumptionIds: [],
              acceptanceCriterionIds: [],
              historicalReceipts: [],
              unsupportedPolicies: [],
              missingLogNotes: [],
              warnings: [],
            } satisfies WorkflowBoardPreviewCliImportResult),
        });
      }
      if (options.label === "workflow board apply cli import") {
        if (state.applyCliImportHandler !== null) {
          return state.applyCliImportHandler(payload.input);
        }
        if (state.applyCliImportError !== null) {
          return Promise.resolve({
            _tag: "Failure",
            cause: Cause.fail(state.applyCliImportError),
          });
        }
        const fallbackCard = makeDetail({
          cardId: WorkflowCardId.make("cli-spec-t3-code"),
          stage: "execute",
        });
        return Promise.resolve({
          _tag: "Success",
          value:
            state.applyCliImportResult ??
            ({
              cardId: fallbackCard.cardId,
              projectId: PROJECT_ID,
              revision: PositiveInt.make(1),
              created: true,
              sourceSpecId: "spec-t3-code",
              tasks: [],
              historicalReceipts: [],
              unsupportedPolicies: [],
              missingLogNotes: [],
              card: fallbackCard,
            } satisfies WorkflowBoardApplyCliImportResult),
        });
      }
      if (options.label === "workflow board create card") {
        return Promise.resolve({
          _tag: "Success",
          value: makeDetail({ cardId: CARD_1 }),
        });
      }
      if (options.label === "workflow board update spec") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(2),
            specDigest: "sha256:digest-2",
            specApprovalStatus: null,
            card: makeDetail({ revision: PositiveInt.make(2) }),
          },
        });
      }
      if (options.label === "workflow board approve spec") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(3),
            specApproval: {
              id: WorkflowBoardApprovalId.make("appr-1"),
              gate: "spec",
              status: "valid",
              revision: PositiveInt.make(3),
              artifactDigest: "sha256:digest-2",
              specDigest: "sha256:digest-2",
              wireframesDigest: null,
              approvedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
              approvedBy: "client",
            },
            card: makeDetail({ revision: PositiveInt.make(3), stage: "plan" }),
          },
        });
      }
      if (options.label === "workflow board explore step") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(2),
            roundIndex: NonNegativeInt.make(1),
            pendingRoundPrompt: false,
            paused: (payload.input as { readonly action?: string }).action === "pause",
            exploration: {
              roundIndex: NonNegativeInt.make(1),
              pendingRoundPrompt: false,
              paused: (payload.input as { readonly action?: string }).action === "pause",
              unresolvedIds: [],
              decisions: [],
            },
            decisions: [],
            card: makeDetail({ revision: PositiveInt.make(2) }),
          },
        });
      }
      if (options.label === "workflow board challenge step") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(3),
            challengeReceipt: {
              status: "complete",
              concerns: [],
              synthesisCount: NonNegativeInt.make(0),
              specDigest: "sha256:digest-1",
              interviewDigest: null,
              manualFallback: null,
              updatedAt: IsoDateTime.make("2026-11-24T00:03:00.000Z"),
            },
            concerns: [],
            card: makeDetail({ revision: PositiveInt.make(3) }),
          },
        });
      }
      if (options.label === "workflow board update plan") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(4),
            planDigest: "sha256:plan-1",
            specApproval: null,
            planApproval: null,
            lenses: [],
            alternatives: [],
            selectedAlternativeId: null,
            selectionRationale: null,
            strategy: "outcomes",
            strategyMetadata: null,
            granularity: "balanced",
            tasks: [],
            acMappings: [],
            card: makeDetail({ revision: PositiveInt.make(4), stage: "plan" }),
          },
        });
      }
      if (options.label === "workflow board approve plan") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(5),
            planApproval: {
              id: WorkflowBoardApprovalId.make("appr-plan-1"),
              gate: "plan",
              status: "valid",
              revision: PositiveInt.make(5),
              artifactDigest: "sha256:plan-1",
              specDigest: "sha256:digest-1",
              wireframesDigest: null,
              planDigest: "sha256:plan-1",
              approvedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
              approvedBy: "client",
            },
            card: makeDetail({ revision: PositiveInt.make(5), stage: "execute" }),
          },
        });
      }
      if (options.label === "workflow board return for revision") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(6),
            stage: "spec",
            specApproval: null,
            planApproval: null,
            history: [],
            card: makeDetail({ revision: PositiveInt.make(6), stage: "spec" }),
          },
        });
      }
      if (options.label === "workflow board pause card") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(6),
            status: "paused",
            card: makeDetail({
              revision: PositiveInt.make(6),
              stage: "execute",
              status: "paused",
            }),
          },
        });
      }
      if (options.label === "workflow board resume card") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(7),
            status: "active",
            reconciledCount: NonNegativeInt.make(1),
            card: makeDetail({
              revision: PositiveInt.make(7),
              stage: "execute",
              status: "active",
            }),
          },
        });
      }
      if (options.label === "workflow board retry step") {
        const input = payload.input as { readonly stepKind: string; readonly targetId?: string };
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(7),
            stepKind: input.stepKind,
            targetId: input.targetId ?? null,
            card: makeDetail({ revision: PositiveInt.make(7), stage: "execute" }),
          },
        });
      }
      if (options.label === "workflow board propose amendment") {
        if (state.proposeAmendmentHandler !== null) {
          return state.proposeAmendmentHandler(payload.input);
        }
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(7),
            stage: "plan",
            amendment: {
              id: "amd-1",
              cardRevision: PositiveInt.make(7),
              targetStage: "plan",
              status: "applied",
              reason: "Updated scope",
              summary: null,
              sourceTaskId: "TSK-01",
              unexpectedFiles: [],
              createdAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
            },
            specApproval: null,
            planApproval: null,
            card: makeDetail({ revision: PositiveInt.make(7), stage: "plan" }),
          },
        });
      }
      if (options.label === "workflow board resolve finding") {
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(8),
            finding: {
              id: "fnd-1",
              severity: "major",
              status: "resolved",
              criterionId: "AC-1",
              file: WorkflowBoardArtifactKey.make("src/core.ts"),
              line: PositiveInt.make(10),
              message: "Handle empty input",
              disposition: "dismiss",
              resolutionNote: "Checked upstream",
              resolvedBy: "client",
              resolvedAt: IsoDateTime.make("2026-11-24T00:06:00.000Z"),
            },
            reviewReport: {
              reviewId: "rev-1",
              stageRunId: "sr-rev-1",
              threadId: ThreadId.make("thread-review-1"),
              status: "completed",
              verdict: "pass",
              summary: "Resolved",
              productSnapshotDigest: "sha256:snap-1",
              findings: [],
              createdAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
              updatedAt: IsoDateTime.make("2026-11-24T00:06:00.000Z"),
              completedAt: IsoDateTime.make("2026-11-24T00:06:00.000Z"),
            },
            card: makeDetail({ revision: PositiveInt.make(8), stage: "execute" }),
          },
        });
      }
      if (options.label === "workflow board record manual acceptance") {
        const input = payload.input as { readonly criterionId: string };
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(9),
            observation: {
              criterionId: input.criterionId,
              method: "manual",
              status: "pass",
              checkIds: [],
              procedure: "Verify UI",
              observationNote: "Verified",
              evidenceRef: null,
              evidenceDigest: null,
              productSnapshotDigest: "sha256:snap-1",
              observedBy: "client",
              observedAt: IsoDateTime.make("2026-11-24T00:07:00.000Z"),
              updatedAt: IsoDateTime.make("2026-11-24T00:07:00.000Z"),
            },
            shipReadiness: {
              status: "ready",
              ready: true,
              productSnapshotDigest: "sha256:snap-1",
              blockers: [],
              deliveryReceipt: null,
            },
            card: makeDetail({ revision: PositiveInt.make(9), stage: "ship" }),
          },
        });
      }
      if (options.label === "workflow board approve delivery") {
        if (state.approveDeliveryError !== null) {
          return Promise.resolve({
            _tag: "Failure",
            cause: Cause.fail(state.approveDeliveryError),
          });
        }
        return Promise.resolve({
          _tag: "Success",
          value: {
            cardId: CARD_1,
            revision: PositiveInt.make(10),
            status: "completed",
            deliveryStatus: "shipped",
            deliveryReceipt: {
              id: "del-1",
              approvalId: WorkflowBoardApprovalId.make("appr-del-1"),
              cardRevision: PositiveInt.make(10),
              productSnapshotDigest: "sha256:snap-1",
              commitSha: null,
              specDigest: "sha256:digest-1",
              planDigest: "sha256:plan-1",
              tasksDigest: "sha256:tasks-1",
              checksDigest: "sha256:checks-1",
              reviewId: "rev-1",
              acVerdictsSummary: "1/1 pass",
              approvedBy: "client",
              approvedAt: IsoDateTime.make("2026-11-24T00:08:00.000Z"),
              status: "shipped",
            },
            deliveryApproval: {
              id: WorkflowBoardApprovalId.make("appr-del-1"),
              gate: "delivery",
              status: "valid",
              revision: PositiveInt.make(10),
              artifactDigest: "sha256:snap-1",
              specDigest: "sha256:digest-1",
              wireframesDigest: null,
              approvedAt: IsoDateTime.make("2026-11-24T00:08:00.000Z"),
              approvedBy: "client",
            },
            card: makeDetail({
              revision: PositiveInt.make(10),
              stage: "ship",
              status: "completed",
              deliveryStatus: "shipped",
            }),
          },
        });
      }
      const startInput = (payload.input ?? {}) as {
        readonly stage?: string;
        readonly taskId?: string;
      };
      const nextThreadId =
        startInput.stage === "review"
          ? ThreadId.make("thread-review-1")
          : startInput.stage === "execute"
            ? ThreadId.make("thread-exec-1")
            : THREAD_ID;
      return Promise.resolve({
        _tag: "Success",
        value: {
          cardId: CARD_1,
          revision: PositiveInt.make(4),
          stage: startInput.stage ?? "plan",
          taskId: startInput.taskId ?? null,
          stageRunId: WorkflowBoardStageRunId.make("sr-1"),
          runId: RunId.make("run-1"),
          threadId: nextThreadId,
          resumed: false,
        },
      });
    })) as never,
}));

vi.mock("../settings/useSettingsProjectGroups", () => ({
  useSettingsProjectGroups: () => [state.multiCheckout ? MULTI_CHECKOUT_GROUP : PROJECT_GROUP],
}));

vi.mock(import("../../state/entities"), async (original) => ({
  ...(await original()),
  useAllEnvironmentProjectSnapshotsReady: () => true,
  useServerConfigs: (() =>
    new Map([
      [
        ENV_ID,
        {
          settings: {
            ...DEFAULT_SERVER_SETTINGS,
            workflowsEnabled: state.workflowsEnabled,
          },
          environment: {
            capabilities: {
              repositoryIdentity: true,
              ...(state.projectWorkflowBoard === undefined
                ? {}
                : { projectWorkflowBoard: state.projectWorkflowBoard }),
            },
          },
        },
      ],
      ...(state.multiCheckout
        ? [
            [
              ENV_2_ID,
              {
                settings: {
                  ...DEFAULT_SERVER_SETTINGS,
                  workflowsEnabled: state.workflowsEnabled,
                },
                environment: {
                  capabilities: {
                    repositoryIdentity: true,
                    projectWorkflowBoard: true,
                  },
                },
              },
            ] as const,
          ]
        : []),
    ])) as never,
  useThreadShellsForProjectRefs: () => [],
  useQuietThreadShell: (() =>
    state.threadShellPresent
      ? { environmentId: ENV_ID, id: THREAD_ID, title: "Plan thread" }
      : null) as never,
}));

vi.mock(import("../../state/environments"), async (original) => ({
  ...(await original()),
  useConnectedEnvironmentIds: () =>
    state.connected ? (state.multiCheckout ? [ENV_ID, ENV_2_ID] : [ENV_ID]) : [],
  usePrimaryEnvironmentId: () => ENV_ID,
  useEnvironmentProviders: () =>
    new Map([
      [ENV_ID, state.providers],
      ...(state.multiCheckout ? ([[ENV_2_ID, state.providers]] as const) : []),
    ]),
}));

vi.mock(import("../../state/shell"), async (original) => ({
  ...(await original()),
  useEnvironmentShellHasSnapshot: () => true,
}));

vi.mock(import("../../state/managedPolicy"), async (original) => ({
  ...(await original()),
  useManagedPolicy: () => undefined,
}));

vi.mock(import("../../state/session"), async (original) => ({
  ...(await original()),
  useEnvironmentScope: () => state.canOperate,
  readEnvironmentScope: () => state.canOperate,
}));

vi.mock(import("../../state/server"), async (original) => {
  const actual = await original();
  return {
    ...actual,
    serverEnvironment: {
      ...actual.serverEnvironment,
      configValueAtom: () => emptyServerConfigAtom as never,
    },
  };
});

vi.mock(import("../../state/filesystem"), async (original) => ({
  ...(await original()),
  useFilesystemReadAccess: () => ({ canReadFiles: true }) as never,
}));

vi.mock(import("../../state/vcs"), async (original) => {
  const actual = await original();
  return {
    ...actual,
    vcsEnvironment: {
      ...actual.vcsEnvironment,
      status: (() => ({ kind: "vcsStatus" })) as never,
      refreshStatus: { kind: "refreshStatus" } as never,
    },
    vcsStatusRevalidation: (() => noopRevalidateAtom) as never,
    vcsActionManager: {
      ...actual.vcsActionManager,
      stateAtom: (() => noopVcsActionStateAtom) as never,
    },
  };
});

vi.mock("../../state/sourceControlActions", () => ({
  useVcsInitAction: () => ({
    isPending: false,
    isAllowed: state.canOperate,
    error: null,
    resetError: vi.fn(),
    run: async () => AsyncResult.success({}),
  }),
  useVcsPullAction: () => ({
    isPending: false,
    isAllowed: state.canOperate,
    error: null,
    resetError: vi.fn(),
    run: async () => AsyncResult.success({ status: "up_to_date", refName: "feat/m3" }),
  }),
  useSourceControlPublishRepositoryAction: () => ({
    isPending: false,
    isAllowed: state.canOperate,
    error: null,
    resetError: vi.fn(),
    run: async () => AsyncResult.success({}),
  }),
  useSourceControlActionRunning: () => false,
  useGitStackedAction: () => ({
    isPending: false,
    isAllowed: state.canOperate,
    error: null,
    resetError: vi.fn(),
    run: async (input: { readonly action: string; readonly filePaths?: ReadonlyArray<string> }) => {
      state.calls.push(["git run stacked action", input]);
      const result: GitRunStackedActionResult = {
        action: input.action as GitRunStackedActionResult["action"],
        branch: { status: "skipped_not_requested" },
        commit:
          input.action === "commit"
            ? { status: "created", commitSha: "abc1234", subject: "Commit changes" }
            : { status: "skipped_not_requested" },
        push:
          input.action === "push"
            ? { status: "pushed", branch: "feat/m3", upstreamBranch: "origin/feat/m3" }
            : { status: "skipped_not_requested" },
        pr:
          input.action === "create_pr"
            ? {
                status: "created",
                number: 42,
                title: "Feature A",
                url: "https://github.com/org/repo/pull/42",
                baseBranch: "main",
                headBranch: "feat/m3",
              }
            : { status: "skipped_not_requested" },
        toast: { title: `${input.action} succeeded`, cta: { kind: "none" } },
      };
      return AsyncResult.success(result);
    },
  }),
}));

vi.mock("../diffs/StyledDiffCodeView", () => ({
  StyledDiffCodeView: (props: {
    readonly items?: ReadonlyArray<{
      readonly id: string;
      readonly fileDiff: { readonly name: string };
    }>;
  }) => (
    <div data-testid="mock-styled-diff">
      {(props.items ?? []).map((item) => (
        <div key={item.id} data-diff-item={item.fileDiff.name}>
          {item.fileDiff.name}
        </div>
      ))}
    </div>
  ),
}));

vi.mock("../ChatMarkdown", () => ({
  default: ({ text }: { readonly text: string }) => <div data-testid="board-markdown">{text}</div>,
}));

vi.mock("../ChatView", () => ({
  default: (props: {
    readonly environmentId: string;
    readonly threadId: string;
    readonly routeKind: string;
    readonly embedded?: unknown;
  }) => (
    <div
      data-testid="embedded-chat-view"
      data-env={props.environmentId}
      data-thread={props.threadId}
      data-route-kind={props.routeKind}
      data-embedded={String(props.embedded !== undefined)}
    >
      {props.threadId}
    </div>
  ),
}));

async function desktop() {
  const width = window.innerWidth;
  const height = window.innerHeight;
  await browserPage.viewport(1280, 800);
  return { [Symbol.asyncDispose]: () => browserPage.viewport(width, height) };
}

describe("WorkflowBoardPage", () => {
  it("reports unsupported server and Labs-disabled states cleanly while keeping Open workflows", async () => {
    setup({ projectWorkflowBoard: undefined });
    await using screen = await render(<WorkflowBoardPage />);

    await expect.element(screen.getByText("Workflow board is not supported")).toBeVisible();
    await expect.element(screen.getByRole("link", { name: "Open workflows" })).toBeVisible();
    expect([...state.requestedQueryKinds]).toEqual([]);

    setup({ projectWorkflowBoard: false });
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByText("Workflow board is not supported")).toBeVisible();
    expect([...state.requestedQueryKinds]).toEqual([]);

    setup({ projectWorkflowBoard: true, workflowsEnabled: false });
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByText("Workflows are off")).toBeVisible();
    await expect.element(screen.getByRole("link", { name: "Settings → Labs" })).toBeVisible();
    expect([...state.requestedQueryKinds]).toEqual([]);

    // Switching checkouts resets Ship filter state, clears selected card, and remounts CardWorkspace
    const shipCardSummary = makeSummary({
      cardId: CARD_1,
      title: "Shipped checkout card",
      stage: "ship",
      status: "completed",
      deliveryStatus: "shipped",
    });
    setup({
      multiCheckout: true,
      search: { stage: "ship", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(1),
        cards: [shipCardSummary],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      cardDetail: makeDetail({
        stage: "ship",
        status: "completed",
        deliveryStatus: "shipped",
      }),
    });
    await screen.rerender(<WorkflowBoardPage />);
    await screen.getByRole("button", { name: "Shipped" }).click();
    await expect
      .element(screen.getByRole("button", { name: "Shipped" }))
      .toHaveAttribute("aria-pressed", "true");
    await screen.getByRole("tab", { name: /History/ }).click();
    await expect.element(screen.getByRole("heading", { name: "Revision ledger" })).toBeVisible();

    await screen.getByRole("combobox", { name: "Checkout" }).click();
    await browserPage.getByRole("option", { name: "Remote" }).click();
    await vi.waitFor(() => {
      expect(state.search.env).toBe(ENV_2_ID);
      expect(state.search.projectId).toBe(PROJECT_2_ID);
      expect(state.search.card).toBeUndefined();
    });
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(screen.getByRole("button", { name: "All" }))
      .toHaveAttribute("aria-pressed", "true");

    // Selecting a card with the same cardId on the second checkout starts at default sub-tab (not stale History)
    state.search = { env: ENV_2_ID, projectId: PROJECT_2_ID, stage: "ship", card: CARD_1 };
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(screen.getByRole("heading", { name: "Delivery readiness & blockers" }))
      .toBeVisible();
  });

  it("renders four columns, blocked cards, completed shelf, and navigates to stage tabs", async () => {
    setup({
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(2),
        cards: [
          makeSummary({
            cardId: CARD_1,
            title: "Alpha card",
            stage: "spec",
            status: "blocked",
            blockerSummary: "Missing decision",
          }),
          makeSummary({
            cardId: CARD_2,
            title: "Beta card",
            stage: "plan",
            specApprovalStatus: "valid",
            nextAction: "start_plan",
          }),
          makeSummary({
            cardId: WorkflowCardId.make("card-done"),
            title: "Shipped feature",
            stage: "ship",
            status: "completed",
          }),
        ],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    const specColumn = screen.getByRole("region", { name: "Spec" });
    const planColumn = screen.getByRole("region", { name: "Plan" });
    await expect.element(specColumn.getByText("Alpha card")).toBeVisible();
    await expect.element(specColumn.getByText("Missing decision").first()).toBeVisible();
    await expect.element(planColumn.getByText("Beta card")).toBeVisible();
    await expect.element(screen.getByText("Shipped feature")).toBeVisible();

    const overviewTab = screen.container.querySelector<HTMLElement>(
      "#workflow-board-tab-overview",
    )!;
    overviewTab.focus();
    await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
    expect(state.search.stage).toBeUndefined();
    expect(document.activeElement?.id).toBe("workflow-board-tab-overview");

    await userEvent.keyboard("{ArrowRight}");
    await vi.waitFor(() => expect(state.search.stage).toBe("spec"));
    expect(document.activeElement?.id).toBe("workflow-board-tab-spec");
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(screen.getByRole("tab", { name: /Spec/ }))
      .toHaveAttribute("aria-selected", "true");

    const specTab = screen.container.querySelector<HTMLElement>("#workflow-board-tab-spec")!;
    specTab.focus();
    await userEvent.keyboard("{End}");
    await vi.waitFor(() => expect(state.search.stage).toBe("ship"));
    expect(document.activeElement?.id).toBe("workflow-board-tab-ship");
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(screen.getByRole("tab", { name: /Ship/ }))
      .toHaveAttribute("aria-selected", "true");

    const shipTab = screen.container.querySelector<HTMLElement>("#workflow-board-tab-ship")!;
    shipTab.focus();
    await userEvent.keyboard("{Home}");
    await vi.waitFor(() => expect(state.search.stage).toBeUndefined());
    expect(document.activeElement?.id).toBe("workflow-board-tab-overview");
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(screen.getByRole("tab", { name: "Overview" }))
      .toHaveAttribute("aria-selected", "true");

    await specColumn.getByRole("button", { name: /Spec/ }).first().click();
    expect(state.search.stage).toBe("spec");
  });

  it("creates a card, drafts and approves spec without sending specDigest, and starts Plan beside ChatView", async () => {
    const approvedDetail = makeDetail({
      revision: PositiveInt.make(3),
      stage: "plan",
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      specApproval: {
        id: WorkflowBoardApprovalId.make("appr-1"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(3),
        artifactDigest: "sha256:digest-1",
        specDigest: "sha256:digest-1",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
        approvedBy: "client",
      },
    });
    setup({
      search: { stage: "spec", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(3),
        cards: [makeSummary({ cardId: CARD_1, title: "Feature A", stage: "spec" })],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      cardDetail: makeDetail(),
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    // Create card modal
    await screen.getByRole("button", { name: "New card" }).click();
    await browserPage.getByLabelText("Title").fill("Board Feature");
    await browserPage.getByLabelText("Initial spec or idea").fill("Implement workflow board");
    await browserPage.getByRole("button", { name: "Create card" }).click();
    await vi.waitFor(() =>
      expect(state.calls.some(([label]) => label === "workflow board create card")).toBe(true),
    );

    // Save spec and approve spec
    await expect.element(screen.getByText(/sha256:digest-1/)).toBeVisible();
    await screen.getByLabelText("Spec title").fill("Updated Feature A");
    await screen.getByRole("button", { name: "Save spec" }).click();
    await vi.waitFor(() =>
      expect(state.calls.some(([label]) => label === "workflow board update spec")).toBe(true),
    );

    await screen.getByRole("button", { name: "Approve spec" }).click();
    await vi.waitFor(() => {
      const approveCall = state.calls.find(([label]) => label === "workflow board approve spec");
      expect(approveCall).toBeDefined();
      expect(approveCall![1]).toEqual({ cardId: CARD_1, expectedRevision: 1 });
      expect("specDigest" in (approveCall![1] as Record<string, unknown>)).toBe(false);
    });

    // After approval, Start Plan opens Plan brief alongside embedded ChatView
    state.cardDetail = {
      ...approvedDetail,
      activeThreadId: THREAD_ID,
      stageRuns: [
        {
          id: WorkflowBoardStageRunId.make("plan-persisted"),
          stage: "plan",
          stableLaunchKey: "plan-persisted",
          status: "completed",
          cardRevision: PositiveInt.make(3),
          runId: null,
          threadId: THREAD_ID,
          projectId: PROJECT_ID,
          workspaceStrategy: { type: "worktree", baseRef: "main", branch: "codex/persisted-plan" },
          worktreePath: "/repo/.worktrees/persisted-plan",
          branch: "codex/persisted-plan",
          modelSelection: { instanceId: ProviderInstanceId.make("devReplay"), model: "dev-replay" },
          runtimeMode: "approval-required",
          interactionMode: "plan",
          lastError: null,
          createdAt: IsoDateTime.make("2026-11-24T00:03:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
          endedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
        },
      ],
      artifactKeys: [
        WorkflowBoardArtifactKey.make("spec.md"),
        WorkflowBoardArtifactKey.make("plan-brief.md"),
      ],
    };
    state.artifact = {
      cardId: CARD_1,
      key: WorkflowBoardArtifactKey.make("plan-brief.md"),
      kind: "plan",
      relativePath: WorkflowBoardArtifactKey.make("plan-brief.md"),
      digest: "sha256:brief-1",
      content: "# Plan Brief: Feature A",
      revision: PositiveInt.make(4),
      updatedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
    };
    state.search = { stage: "plan", card: CARD_1 };
    await screen.rerender(<WorkflowBoardPage />);

    await expect
      .element(screen.getByRole("combobox", { name: "Workspace" }))
      .toHaveTextContent("New worktree");
    await screen.getByRole("button", { name: "Resume Plan" }).click();
    await vi.waitFor(() => {
      const startCall = state.calls.find(([label]) => label === "workflow board start stage");
      expect(startCall).toBeDefined();
      expect(startCall![1]).toMatchObject({
        cardId: CARD_1,
        stage: "plan",
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
        workspaceStrategy: { type: "worktree", baseRef: "main", branch: "codex/persisted-plan" },
        runtimeMode: "approval-required",
        interactionMode: "plan",
      });
    });

    await expect
      .element(screen.getByTestId("board-markdown"))
      .toHaveTextContent("# Plan Brief: Feature A");
    const embeddedChat = screen.getByTestId("embedded-chat-view");
    await expect.element(embeddedChat).toBeVisible();
    await expect.element(embeddedChat).toHaveAttribute("data-thread", THREAD_ID);
    await expect.element(embeddedChat).toHaveAttribute("data-route-kind", "server");
    await expect.element(embeddedChat).toHaveAttribute("data-embedded", "true");
    expect(screen.container.querySelector("[data-side-chat-pane]")).not.toBeNull();
  });

  it("gates the sidebar folder workflow board entry behind onWorkflowBoard", async () => {
    const onWorkflowBoard = vi.fn();
    await using screen = await render(
      <ul>
        <SidebarProjectFolderHeader
          group={PROJECT_GROUP}
          collapsed={false}
          hiddenThreads={[]}
          onToggle={vi.fn()}
          onNewThread={vi.fn()}
          onContextMenu={vi.fn()}
        />
      </ul>,
    );

    await expect
      .element(screen.getByRole("button", { name: "Workflow board for t3code" }))
      .not.toBeInTheDocument();

    await screen.rerender(
      <ul>
        <SidebarProjectFolderHeader
          group={PROJECT_GROUP}
          collapsed={false}
          hiddenThreads={[]}
          onToggle={vi.fn()}
          onNewThread={vi.fn()}
          onWorkflowBoard={onWorkflowBoard}
          onContextMenu={vi.fn()}
        />
      </ul>,
    );

    const boardButton = screen.getByRole("button", { name: "Workflow board for t3code" });
    await boardButton.click({ force: true });
    expect(onWorkflowBoard).toHaveBeenCalledWith(PROJECT_GROUP);
  });

  it("disables Create card and Start Plan when Existing worktree has an empty path", async () => {
    const approvedDetail = makeDetail({
      revision: PositiveInt.make(3),
      stage: "plan",
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      specApproval: {
        id: WorkflowBoardApprovalId.make("appr-1"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(3),
        artifactDigest: "sha256:digest-1",
        specDigest: "sha256:digest-1",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
        approvedBy: "client",
      },
    });
    setup({
      search: { stage: "plan", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(3),
        cards: [
          makeSummary({
            cardId: CARD_1,
            title: "Feature A",
            stage: "plan",
            specApprovalStatus: "valid",
          }),
        ],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      cardDetail: approvedDetail,
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    // Create card dialog with Existing worktree and empty path
    await screen.getByRole("button", { name: "New card" }).click();
    await browserPage.getByLabelText("Title").fill("Worktree card");
    await browserPage.getByRole("dialog").getByRole("combobox", { name: "Workspace" }).click();
    await browserPage.getByRole("option", { name: "Existing worktree" }).click();

    await expect
      .element(
        browserPage.getByRole("dialog").getByText("Enter an existing worktree path to continue."),
      )
      .toBeVisible();
    await expect
      .element(browserPage.getByRole("dialog").getByLabelText("Worktree path"))
      .toHaveAttribute("aria-invalid", "true");
    await expect.element(browserPage.getByRole("button", { name: "Create card" })).toBeDisabled();
    expect(state.calls).toHaveLength(0);

    await browserPage
      .getByRole("dialog")
      .getByLabelText("Worktree path")
      .fill("/repo/.worktrees/wt-1");
    await expect
      .element(
        browserPage.getByRole("dialog").getByText("Enter an existing worktree path to continue."),
      )
      .not.toBeInTheDocument();
    await expect.element(browserPage.getByRole("button", { name: "Create card" })).toBeEnabled();
    await browserPage.getByRole("button", { name: "Cancel" }).click();

    // Card workspace Start Plan with Existing worktree and empty path
    await screen.getByRole("combobox", { name: "Workspace" }).click();
    await browserPage.getByRole("option", { name: "Existing worktree" }).click();

    await expect
      .element(screen.getByText("Enter an existing worktree path to continue."))
      .toBeVisible();
    await expect
      .element(screen.getByLabelText("Worktree path"))
      .toHaveAttribute("aria-invalid", "true");
    await expect.element(screen.getByRole("button", { name: "Start Plan" })).toBeDisabled();
    expect(state.calls).toHaveLength(0);

    await screen.getByLabelText("Worktree path").fill("/repo/.worktrees/wt-plan");
    await expect
      .element(screen.getByText("Enter an existing worktree path to continue."))
      .not.toBeInTheDocument();
    await expect.element(screen.getByRole("button", { name: "Start Plan" })).toBeEnabled();
    await screen.getByRole("button", { name: "Start Plan" }).click();
    await vi.waitFor(() => {
      const startCall = state.calls.find(([label]) => label === "workflow board start stage");
      expect(startCall).toBeDefined();
      expect(startCall![1]).toMatchObject({
        workspaceStrategy: {
          type: "existing_worktree",
          worktreePath: "/repo/.worktrees/wt-plan",
        },
      });
    });
  });

  it("hydrates default modelSelection when providers load after initial mount", async () => {
    const approvedWithoutModel = makeDetail({
      revision: PositiveInt.make(3),
      stage: "plan",
      modelSelection: null,
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      specApproval: {
        id: WorkflowBoardApprovalId.make("appr-1"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(3),
        artifactDigest: "sha256:digest-1",
        specDigest: "sha256:digest-1",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
        approvedBy: "client",
      },
    });
    setup({
      providers: [],
      search: { stage: "plan", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(3),
        cards: [
          makeSummary({
            cardId: CARD_1,
            title: "Feature A",
            stage: "plan",
            specApprovalStatus: "valid",
          }),
        ],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      cardDetail: approvedWithoutModel,
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    await expect.element(screen.getByRole("button", { name: "Start Plan" })).toBeDisabled();

    state.providers = [DEV_REPLAY_PROVIDER];
    await screen.rerender(<WorkflowBoardPage />);

    await expect.element(screen.getByRole("button", { name: "Start Plan" })).toBeEnabled();
    await screen.getByRole("button", { name: "Start Plan" }).click();
    await vi.waitFor(() => {
      const startCall = state.calls.find(([label]) => label === "workflow board start stage");
      expect(startCall).toBeDefined();
      expect(startCall![1]).toMatchObject({
        cardId: CARD_1,
        stage: "plan",
        modelSelection: { instanceId: "devReplay", model: "dev-replay" },
      });
    });
  });

  it("allows clearing and editing Base ref in New worktree mode without snapping back to HEAD", async () => {
    setup({
      search: { stage: "spec", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(1),
        cards: [makeSummary({ cardId: CARD_1, title: "Feature A", stage: "spec" })],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      cardDetail: makeDetail(),
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    await screen.getByRole("button", { name: "New card" }).click();
    await browserPage.getByLabelText("Title").fill("Worktree Base Ref Card");
    await browserPage.getByRole("dialog").getByRole("combobox", { name: "Workspace" }).click();
    await browserPage.getByRole("option", { name: "New worktree" }).click();

    const baseRefInput = browserPage.getByRole("dialog").getByLabelText("Base ref");
    await expect.element(baseRefInput).toHaveValue("HEAD");
    await baseRefInput.fill("");
    await expect.element(baseRefInput).toHaveValue("");
    await expect
      .element(browserPage.getByRole("dialog").getByText("Enter a base ref to continue."))
      .toBeVisible();
    await expect.element(baseRefInput).toHaveAttribute("aria-invalid", "true");
    await expect.element(browserPage.getByRole("button", { name: "Create card" })).toBeDisabled();

    await baseRefInput.fill("main");
    await expect.element(baseRefInput).toHaveValue("main");
    await expect.element(browserPage.getByRole("button", { name: "Create card" })).toBeEnabled();
    await browserPage.getByRole("button", { name: "Create card" }).click();
    await vi.waitFor(() => {
      const createCall = state.calls.find(([label]) => label === "workflow board create card");
      expect(createCall).toBeDefined();
      expect(createCall![1]).toMatchObject({
        workspaceStrategy: { type: "worktree", baseRef: "main" },
        wireframesEnabled: false,
        wireframesIndex: null,
      });
    });
  });

  it("supports M2 Spec exploration, sandboxed wireframe preview, non-UI clear, and bounded challenge fallback", async () => {
    const wireframeDetail = makeDetail({
      spec: {
        title: "UI Feature",
        brief: "Build settings screen",
        sourceType: "codebase",
        assistanceMode: "guided",
        intakeGoal: "change",
        intakeAnalysis: {
          suppliedDecisions: ["Core UI requirement supplied"],
          inferredDecisions: ["Detected package.json"],
          missingDecisions: ["Confirm layout density"],
          sourceDigest: "sha256:src-1",
        },
        acceptanceCriteria: [{ id: "AC-1", text: "Preview renders safely" }],
        constraints: [],
        sourceRefs: ["package.json"],
        wireframesIndex: WorkflowBoardArtifactKey.make("wireframes/index.html"),
        specDigest: "sha256:digest-wf",
        updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      wireframesIndex: WorkflowBoardArtifactKey.make("wireframes/index.html"),
      exploration: {
        roundIndex: NonNegativeInt.make(1),
        pendingRoundPrompt: false,
        paused: false,
        unresolvedIds: ["DEC-1"],
        decisions: [
          {
            id: "DEC-1",
            prompt: "Choose layout density",
            status: "unresolved",
            lastAction: null,
            answer: null,
            explanation: null,
            recommendation: null,
            updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
          },
        ],
      },
      challengeReceipt: {
        status: "in_progress",
        concerns: [
          {
            id: "CON-1",
            summary: "Verify sandbox isolation",
            whyMatters: "Prevents session access",
            evidence: "BrowserDocumentFrame.tsx",
            assumption: "Opaque null origin",
            disposition: "unresolved",
            answer: null,
            reason: null,
          },
        ],
        synthesisCount: NonNegativeInt.make(0),
        specDigest: "sha256:digest-wf",
        interviewDigest: null,
        manualFallback: null,
        updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
    });

    setup({
      search: { stage: "spec", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(1),
        cards: [makeSummary({ cardId: CARD_1, title: "UI Feature", stage: "spec" })],
        generatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
      cardDetail: wireframeDetail,
      artifact: {
        cardId: CARD_1,
        key: WorkflowBoardArtifactKey.make("wireframes/index.html"),
        kind: "wireframes",
        relativePath: WorkflowBoardArtifactKey.make("wireframes/index.html"),
        digest: "sha256:html-1",
        content: "<!doctype html><html><body><h1>Mock Wireframe</h1></body></html>",
        revision: PositiveInt.make(1),
        updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      },
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    // Brownfield intake scan rendered
    await expect.element(screen.getByText("Brownfield intake scan")).toBeVisible();
    await expect.element(screen.getByText("Detected package.json")).toBeVisible();

    // Sandboxed wireframe iframe strictly uses allow-scripts allow-forms without allow-same-origin
    await vi.waitFor(() => {
      const iframe = screen.container.querySelector("iframe");
      expect(iframe).not.toBeNull();
      expect(iframe?.getAttribute("sandbox")).toBe("allow-scripts allow-forms");
    });

    // Preserve active History tab across revision bump (key={cardId})
    await screen.getByRole("tab", { name: /History/ }).click();
    await expect.element(screen.getByText("Revision ledger")).toBeVisible();
    state.cardDetail = { ...wireframeDetail, revision: PositiveInt.make(2) };
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByText("Revision ledger")).toBeVisible();

    await screen.getByRole("tab", { name: "Spec" }).click();

    // Pause exploration preserves answers without approving
    await screen.getByRole("button", { name: "Pause" }).click();
    await vi.waitFor(() => {
      const pauseCall = state.calls.find(([label]) => label === "workflow board explore step");
      expect(pauseCall).toBeDefined();
      expect(pauseCall![1]).toMatchObject({ action: "pause" });
    });

    // Skip wireframes (non-UI) sends wireframesIndex: null
    await screen.getByRole("button", { name: "Skip wireframes (non-UI)" }).click();
    await vi.waitFor(() => {
      const skipCall = state.calls.find(([label]) => label === "workflow board update spec");
      expect(skipCall).toBeDefined();
      expect(skipCall![1]).toMatchObject({ wireframesIndex: null });
    });

    // Challenge response
    await screen.getByRole("combobox", { name: "Disposition for CON-1" }).click();
    await browserPage.getByRole("option", { name: "Answer" }).click();
    await screen
      .getByLabelText("Answer for CON-1")
      .fill("HtmlRenderDocument omits allow-same-origin.");
    await screen.getByRole("button", { name: "Save challenge responses" }).click();
    await vi.waitFor(() => {
      const resolveCall = state.calls.find(([label]) => label === "workflow board challenge step");
      expect(resolveCall).toBeDefined();
      expect(resolveCall![1]).toMatchObject({
        action: "resolve",
        answers: [
          {
            concernId: "CON-1",
            disposition: "answer",
            response: "HtmlRenderDocument omits allow-same-origin.",
          },
        ],
      });
    });

    // Manual 3-question fallback gating requires 0 unresolved concerns & exact "I acknowledge"
    state.cardDetail = {
      ...wireframeDetail,
      challengeReceipt: {
        ...wireframeDetail.challengeReceipt!,
        status: "resolved",
        concerns: [],
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await screen.getByRole("button", { name: "Manual fallback" }).click();
    await expect
      .element(screen.getByRole("button", { name: "Submit manual fallback" }))
      .toBeDisabled();
    await screen.getByLabelText(/1\. What failure mode/).fill("Sandbox escape");
    await screen.getByLabelText(/2\. Which acceptance criteria/).fill("AC-1 iframe attribute");
    await screen.getByLabelText(/3\. How will Plan verification/).fill("Playwright assertion");
    await screen.getByLabelText(/to confirm manual challenge completion/).fill("I acknowledge");
    await expect
      .element(screen.getByRole("button", { name: "Submit manual fallback" }))
      .toBeEnabled();
    await screen.getByRole("button", { name: "Submit manual fallback" }).click();
    await vi.waitFor(() => {
      const manualCall = state.calls
        .filter(([label]) => label === "workflow board challenge step")
        .at(-1);
      expect(manualCall).toBeDefined();
      expect(manualCall![1]).toMatchObject({
        action: "manualFallback",
        acknowledgementToken: "I acknowledge",
        unresolvedConcernIds: [],
        answers: ["Sandbox escape", "AC-1 iframe attribute", "Playwright assertion"],
      });
    });
  });

  it("supports Plan authoring (6 lenses, 2 alternatives, strategy, DAG, AC mapper), stale badges, and drag/button/keyboard transition parity", async () => {
    const planDetail = makeDetail({
      revision: PositiveInt.make(4),
      stage: "plan",
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      specApproval: {
        id: WorkflowBoardApprovalId.make("appr-1"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(3),
        artifactDigest: "sha256:digest-1",
        specDigest: "sha256:digest-1",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
        approvedBy: "client",
      },
      planApprovalStatus: "stale",
      planApproval: {
        id: WorkflowBoardApprovalId.make("appr-plan-stale"),
        gate: "plan",
        status: "stale",
        revision: PositiveInt.make(3),
        artifactDigest: "sha256:plan-0",
        specDigest: "sha256:digest-1",
        wireframesDigest: null,
        planDigest: "sha256:plan-0",
        approvedAt: IsoDateTime.make("2026-11-24T00:02:30.000Z"),
        approvedBy: "client",
      },
      plan: {
        lenses: [
          { lens: "ux", applicable: false, reason: "CLI only" },
          { lens: "functional", applicable: true, summary: "Covers AC-1" },
          { lens: "architecture", applicable: true, summary: "Modular handlers" },
          { lens: "quality", applicable: true, summary: "Automated suite" },
          { lens: "infrastructure", applicable: false, reason: "Local SQLite" },
          { lens: "delivery", applicable: true, summary: "Single task" },
        ],
        alternatives: [
          { id: "alt-1", title: "Direct", summary: "Simple path", tradeoffs: "Low risk" },
          { id: "alt-2", title: "Layered", summary: "Extra adapter", tradeoffs: "Higher cost" },
        ],
        alternativesRevisionCount: NonNegativeInt.make(0),
        selectedAlternativeId: "alt-1",
        selectionRationale: "Lower complexity and minimal surface area.",
        strategy: "outcomes",
        strategyRationale: null,
        strategyMetadata: null,
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-01",
            title: "Implement core",
            filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/core.ts")],
            testingMethod: "test-first",
          },
        ],
        checks: [{ id: "CHK-01", argv: ["vp", "test", "run"] }],
        acMappings: [{ criterionId: "AC-1", method: "automated", checkIds: ["CHK-01"] }],
        planDigest: "sha256:plan-1",
        tasksDigest: "sha256:tasks-1",
        updatedAt: IsoDateTime.make("2026-11-24T00:03:00.000Z"),
      },
    });

    setup({
      search: { stage: "plan", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(4),
        cards: [
          makeSummary({
            cardId: CARD_1,
            title: "Feature A",
            stage: "plan",
            specApprovalStatus: "valid",
            planApprovalStatus: "stale",
          }),
        ],
        generatedAt: IsoDateTime.make("2026-11-24T00:03:00.000Z"),
      },
      cardDetail: planDetail,
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    await expect.element(screen.getByText("Plan-only edit preserves Spec approval")).toBeVisible();
    await expect.element(screen.getByText("Plan approval is stale")).toBeVisible();

    await screen.getByRole("button", { name: "Save plan" }).click();
    await vi.waitFor(() => {
      const savePlanCall = state.calls.find(([label]) => label === "workflow board update plan");
      expect(savePlanCall).toBeDefined();
      expect(savePlanCall![1]).toMatchObject({
        selectedAlternativeId: "alt-1",
        selectionRationale: "Lower complexity and minimal surface area.",
        strategy: "outcomes",
        granularity: "balanced",
      });
    });

    await screen.getByRole("button", { name: "Approve plan" }).click();
    await vi.waitFor(() => {
      expect(state.calls.some(([label]) => label === "workflow board approve plan")).toBe(true);
    });

    // Single-field external AC mappings (externalRef-only and externalSource-only), renamed selected alternative ID, and empty check.id guard
    state.calls = [];
    const twoCriteriaPlanDetail = makeDetail({
      ...planDetail,
      spec: {
        ...planDetail.spec,
        acceptanceCriteria: [
          { id: "AC-1", text: "First criterion" },
          { id: "AC-2", text: "Second criterion" },
        ],
      },
      plan: {
        ...planDetail.plan!,
        acMappings: [
          { criterionId: "AC-1", method: "automated", checkIds: ["CHK-01"] },
          { criterionId: "AC-2", method: "automated", checkIds: ["CHK-01"] },
        ],
      },
    });
    state.cardDetail = twoCriteriaPlanDetail;
    await screen.rerender(<WorkflowBoardPage />);

    await screen.getByLabelText("Evidence method for AC-1").click();
    await browserPage.getByRole("option", { name: "External" }).click();
    await screen.getByLabelText("External reference for AC-1").fill("JIRA-42");

    await screen.getByLabelText("Evidence method for AC-2").click();
    await browserPage.getByRole("option", { name: "External" }).click();
    await screen.getByLabelText("External source for AC-2").fill("GitHub Actions");

    // Rename selected Alternative 1 ID and verify selectedAlternativeId stays synchronized
    await screen.getByLabelText("Alternative 1 ID").fill("alt-1-renamed");

    // Clearing Check 1 ID surfaces inline alert and blocks Save plan pre-submit
    await screen.getByLabelText("Check 1 ID").fill("");
    await expect.element(screen.getByText("Check ID cannot be empty.")).toBeVisible();
    await screen.getByRole("button", { name: "Save plan" }).click();
    await expect
      .element(screen.getByText("Every planned check requires a non-empty ID."))
      .toBeVisible();
    expect(state.calls).toHaveLength(0);

    // Restore Check 1 ID and confirm single-field external mappings + renamed selectedAlternativeId save cleanly
    await screen.getByLabelText("Check 1 ID").fill("CHK-01");
    await screen.getByRole("button", { name: "Save plan" }).click();
    await vi.waitFor(() => {
      const savePlanCall = state.calls.find(([label]) => label === "workflow board update plan");
      expect(savePlanCall).toBeDefined();
      expect(savePlanCall![1]).toMatchObject({
        selectedAlternativeId: "alt-1-renamed",
        acMappings: [
          { criterionId: "AC-1", method: "external", externalRef: "JIRA-42" },
          { criterionId: "AC-2", method: "external", externalSource: "GitHub Actions" },
        ],
      });
      const payload = savePlanCall![1] as {
        readonly acMappings: ReadonlyArray<Record<string, unknown>>;
      };
      expect("externalSource" in payload.acMappings[0]!).toBe(false);
      expect("externalRef" in payload.acMappings[1]!).toBe(false);
    });

    // Overview button, keyboard, and pointer drag parity
    state.search = {};
    state.calls = [];
    state.snapshot = {
      projectId: PROJECT_ID,
      revision: NonNegativeInt.make(5),
      cards: [
        makeSummary({
          cardId: CARD_1,
          title: "Feature A",
          stage: "plan",
          specApprovalStatus: "valid",
        }),
      ],
      generatedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
    };
    await screen.rerender(<WorkflowBoardPage />);

    // Button parity: Return opens TransitionDialog and submits returnForRevision
    await screen.getByRole("button", { name: "Return Feature A for revision" }).click();
    await expect.element(browserPage.getByRole("alertdialog")).toBeVisible();
    await browserPage.getByLabelText("Reason for revision").fill("Tighten acceptance criteria");
    await browserPage.getByRole("button", { name: "Confirm return for revision" }).click();
    await vi.waitFor(() => {
      const returnCall = state.calls.find(
        ([label]) => label === "workflow board return for revision",
      );
      expect(returnCall).toBeDefined();
      expect(returnCall![1]).toMatchObject({
        cardId: CARD_1,
        targetStage: "spec",
        reason: "Tighten acceptance criteria",
      });
    });

    // Keyboard parity: Alt+ArrowRight triggers Approve plan confirmation dialog
    state.search = {};
    state.calls = [];
    await screen.rerender(<WorkflowBoardPage />);
    const cardButton = screen.container.querySelector<HTMLElement>(
      `[data-board-card-id="${CARD_1}"] button:not([data-board-drag-handle])`,
    )!;
    cardButton.focus();
    await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
    await expect.element(browserPage.getByRole("alertdialog")).toBeVisible();
    await expect.element(browserPage.getByText("Approve plan and move to Execute")).toBeVisible();
    await browserPage.getByRole("button", { name: "Confirm plan approval" }).click();
    await vi.waitFor(() => {
      expect(state.calls.some(([label]) => label === "workflow board approve plan")).toBe(true);
    });

    // Pointer drag parity: drag card across columns to Execute column
    state.search = {};
    state.calls = [];
    await screen.rerender(<WorkflowBoardPage />);
    const dragHandle = screen.container.querySelector<HTMLElement>(
      `[data-board-drag-handle="${CARD_1}"]`,
    )!;
    const executeColumn = screen.container.querySelector<HTMLElement>(
      '[data-board-stage-column="execute"]',
    )!;
    const startRect = dragHandle.getBoundingClientRect();
    const endRect = executeColumn.getBoundingClientRect();
    const startX = startRect.left + startRect.width / 2;
    const startY = startRect.top + startRect.height / 2;
    const endX = endRect.left + endRect.width / 2;
    const endY = endRect.top + 40;
    const pointer = (type: string, x: number, y: number) =>
      new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        isPrimary: true,
        button: 0,
        buttons: type === "pointerup" ? 0 : 1,
        clientX: x,
        clientY: y,
      });
    using _release = {
      [Symbol.dispose]: () => document.dispatchEvent(pointer("pointerup", endX, endY)),
    };
    dragHandle.dispatchEvent(pointer("pointerdown", startX, startY));
    document.dispatchEvent(pointer("pointermove", startX + 20, startY));
    await vi.waitFor(() => {
      expect(
        screen.container.querySelector(`[data-board-card-id="${CARD_1}"]`)?.className ?? "",
      ).toContain("opacity-50");
    });
    document.dispatchEvent(pointer("pointermove", endX, endY));
    await vi.waitFor(() => {
      expect(executeColumn.className).toContain("border-primary");
    });
    document.dispatchEvent(pointer("pointerup", endX, endY));
    await expect.element(browserPage.getByRole("alertdialog")).toBeVisible();
    await browserPage.getByRole("button", { name: "Cancel" }).click();
    await expect.element(browserPage.getByRole("alertdialog")).not.toBeInTheDocument();
    expect(state.calls).toHaveLength(0);

    // Releasing drag inside the card's own source column cancels cleanly without opening dialog
    const planColumn = screen.container.querySelector<HTMLElement>(
      '[data-board-stage-column="plan"]',
    )!;
    const planRect = planColumn.getBoundingClientRect();
    const insidePlanX = planRect.left + planRect.width / 2;
    const insidePlanY = planRect.top + 40;
    dragHandle.dispatchEvent(pointer("pointerdown", startX, startY));
    document.dispatchEvent(pointer("pointermove", startX + 20, startY));
    await vi.waitFor(() => {
      expect(
        screen.container.querySelector(`[data-board-card-id="${CARD_1}"]`)?.className ?? "",
      ).toContain("opacity-50");
    });
    document.dispatchEvent(pointer("pointermove", insidePlanX, insidePlanY));
    document.dispatchEvent(pointer("pointerup", insidePlanX, insidePlanY));
    await expect.element(browserPage.getByRole("alertdialog")).not.toBeInTheDocument();
    expect(state.calls).toHaveLength(0);
  });

  it("supports M3 Execute DAG tasks, unexpected-edit amendment, recovery, planned checks, fresh-thread review, and finding dispositions", async () => {
    const executeDetail = makeDetail({
      revision: PositiveInt.make(6),
      stage: "execute",
      status: "blocked",
      blockerSummary: "Unexpected edits preserved",
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      planApprovedRevision: PositiveInt.make(4),
      planApprovalStatus: "valid",
      planApproval: {
        id: WorkflowBoardApprovalId.make("appr-plan"),
        gate: "plan",
        status: "valid",
        revision: PositiveInt.make(4),
        artifactDigest: "sha256:plan-1",
        specDigest: "sha256:digest-1",
        wireframesDigest: null,
        planDigest: "sha256:plan-1",
        approvedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
        approvedBy: "client",
      },
      plan: {
        lenses: [],
        alternatives: [],
        alternativesRevisionCount: NonNegativeInt.make(0),
        selectedAlternativeId: "alt-1",
        selectionRationale: "Simple",
        strategy: "outcomes",
        strategyRationale: null,
        strategyMetadata: null,
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-01",
            title: "Core change",
            filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/core.ts")],
            blocks: ["TSK-02"],
            testingMethod: "test-first",
            status: "completed",
          },
          {
            taskId: "TSK-02",
            title: "Follow-up task",
            dependsOn: ["TSK-01"],
            filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/next.ts")],
            testingMethod: "test-first",
            status: "todo",
          },
        ],
        checks: [{ id: "CHK-01", criterionId: "AC-1", argv: ["node", "-e", "process.exit(0)"] }],
        acMappings: [{ criterionId: "AC-1", method: "automated", checkIds: ["CHK-01"] }],
        planDigest: "sha256:plan-1",
        tasksDigest: "sha256:tasks-1",
        updatedAt: IsoDateTime.make("2026-11-24T00:03:00.000Z"),
      },
      taskExecutions: [
        {
          taskId: "TSK-01",
          status: "completed",
          threadId: ThreadId.make("thread-exec-0"),
          runId: RunId.make("run-0"),
          stageRunId: "sr-exec-0",
          worktreePath: "/repo/.worktrees/card-1",
          branch: "feat/card-1",
          attempt: PositiveInt.make(1),
          startedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
          endedAt: IsoDateTime.make("2026-11-24T00:04:30.000Z"),
          lastError: null,
        },
      ],
      fileContractReports: [
        {
          id: "fcr-1",
          taskId: "TSK-01",
          threadId: ThreadId.make("thread-exec-0"),
          runId: RunId.make("run-0"),
          status: "violation_pending",
          changedFiles: [
            WorkflowBoardArtifactKey.make("src/core.ts"),
            WorkflowBoardArtifactKey.make("src/extra.ts"),
          ],
          allowedFiles: [WorkflowBoardArtifactKey.make("src/core.ts")],
          unexpectedFiles: [WorkflowBoardArtifactKey.make("src/extra.ts")],
          forbiddenFiles: [],
          preservedInWorktree: true,
          blockingReason: "Unexpected file src/extra.ts edited",
          updatedAt: IsoDateTime.make("2026-11-24T00:04:30.000Z"),
        },
      ],
      checkReceipts: [
        {
          checkId: "CHK-01",
          criterionId: "AC-1",
          ownerTaskId: "TSK-01",
          argv: ["node", "-e", "process.exit(0)"],
          description: "Run core test",
          status: "passed",
          exitCode: 0,
          stdoutExcerpt: "ok",
          stderrExcerpt: null,
          logRef: ".legion/workflows/card-1/logs/CHK-01.log",
          productSnapshotDigest: "sha256:snap-old",
          executedAt: IsoDateTime.make("2026-11-24T00:04:40.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T00:04:40.000Z"),
        },
      ],
      reviewReport: {
        reviewId: "rev-1",
        stageRunId: "sr-rev-1",
        threadId: ThreadId.make("thread-review-1"),
        status: "completed",
        verdict: "findings",
        summary: "Found 1 issue",
        productSnapshotDigest: "sha256:snap-new",
        findings: [
          {
            id: "fnd-1",
            severity: "major",
            status: "open",
            criterionId: "AC-1",
            file: WorkflowBoardArtifactKey.make("src/core.ts"),
            line: PositiveInt.make(10),
            message: "Handle empty input",
            disposition: "unresolved",
            resolutionNote: null,
            resolvedBy: null,
            resolvedAt: null,
          },
        ],
        createdAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-24T00:05:30.000Z"),
        completedAt: IsoDateTime.make("2026-11-24T00:05:30.000Z"),
      },
      shipReadiness: {
        status: "blocked",
        ready: false,
        productSnapshotDigest: "sha256:snap-new",
        blockers: [],
        deliveryReceipt: null,
      },
    });

    const diffResult: WorkflowBoardGetCardDiffResult = {
      cardId: CARD_1,
      worktreePath: "/repo/.worktrees/card-1",
      baseRef: "main",
      productSnapshotDigest: "sha256:snap-new",
      diffDigest: "sha256:diff-1",
      files: [
        {
          path: WorkflowBoardArtifactKey.make("src/core.ts"),
          previousPath: null,
          changeType: "change",
          additions: NonNegativeInt.make(2),
          deletions: NonNegativeInt.make(1),
          patch:
            "diff --git a/src/core.ts b/src/core.ts\n--- a/src/core.ts\n+++ b/src/core.ts\n@@ -1 +1,2 @@\n-a\n+a\n+b\n",
          contractStatus: "allowed",
        },
        {
          path: WorkflowBoardArtifactKey.make("src/extra.ts"),
          previousPath: null,
          changeType: "new",
          additions: NonNegativeInt.make(1),
          deletions: NonNegativeInt.make(0),
          patch:
            "diff --git a/src/extra.ts b/src/extra.ts\n--- /dev/null\n+++ b/src/extra.ts\n@@ -0,0 +1 @@\n+x\n",
          contractStatus: "unexpected",
        },
      ],
      truncated: false,
      generatedAt: IsoDateTime.make("2026-11-24T00:05:30.000Z"),
    };

    setup({
      search: { stage: "execute", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(6),
        cards: [
          makeSummary({
            cardId: CARD_1,
            title: "Feature A",
            stage: "execute",
            status: "blocked",
            blockerSummary: "Unexpected edits preserved",
            specApprovalStatus: "valid",
            planApprovalStatus: "valid",
          }),
        ],
        generatedAt: IsoDateTime.make("2026-11-24T00:05:30.000Z"),
      },
      cardDetail: executeDetail,
      cardDiff: diffResult,
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    // Unexpected edit banner preserves worktree edits and opens pre-filled amendment
    await expect.element(screen.getByText(/Unexpected edits preserved for review/)).toBeVisible();
    await expect.element(screen.getByText(/preserved intact in the worktree/)).toBeVisible();
    const violationCard = screen.container.querySelector(
      '[data-contract-violation-task="TSK-01"]',
    )!;
    expect(violationCard.textContent).toContain("src/extra.ts");

    // Stale check receipt shows Stale badge & Re-run check works
    const checkItem = screen.container.querySelector('[data-check-id="CHK-01"]')!;
    expect(checkItem.textContent).toContain("Stale");
    await screen.getByRole("button", { name: "Re-run check" }).click();
    await vi.waitFor(() => {
      const retryCall = state.calls.find(([label]) => label === "workflow board retry step");
      expect(retryCall).toBeDefined();
      expect(retryCall![1]).toMatchObject({ stepKind: "check", targetId: "CHK-01" });
    });

    // Propose plan amendment from violation banner includes unexpectedFiles
    await screen.getByRole("button", { name: "Propose plan amendment" }).nth(1).click();
    await screen.getByRole("button", { name: "Submit amendment" }).click();
    await vi.waitFor(() => {
      const amendCall = state.calls.find(([label]) => label === "workflow board propose amendment");
      expect(amendCall).toBeDefined();
      expect(amendCall![1]).toMatchObject({
        targetStage: "plan",
        sourceTaskId: "TSK-01",
        unexpectedFiles: ["src/extra.ts"],
      });
    });

    // Finding resolution validates reason required for dismiss, then resolves with reviewId
    await screen.getByRole("combobox", { name: "Disposition for fnd-1" }).click();
    await browserPage.getByRole("option", { name: "Dismiss" }).click();
    await screen.getByRole("button", { name: "Resolve finding" }).click();
    await expect
      .element(
        screen.getByText("A reason is required when dismissing or accepting risk for a finding."),
      )
      .toBeVisible();
    await screen.getByLabelText("Resolution reason for fnd-1").fill("Checked upstream");
    await screen.getByRole("button", { name: "Resolve finding" }).click();
    await vi.waitFor(() => {
      const resolveCall = state.calls.find(([label]) => label === "workflow board resolve finding");
      expect(resolveCall).toBeDefined();
      expect(resolveCall![1]).toMatchObject({
        reviewId: "rev-1",
        findingId: "fnd-1",
        disposition: "dismiss",
        reason: "Checked upstream",
      });
    });

    // .git/* unexpected file is rejected inline without unhandled rejection, and unchecking allows submission
    state.calls = [];
    state.cardDetail = {
      ...executeDetail,
      fileContractReports: [
        {
          ...executeDetail.fileContractReports![0]!,
          changedFiles: [
            WorkflowBoardArtifactKey.make("src/core.ts"),
            WorkflowBoardArtifactKey.make(".git/config"),
          ],
          unexpectedFiles: [WorkflowBoardArtifactKey.make(".git/config")],
          blockingReason: "Unexpected file .git/config edited",
        },
      ],
    };
    await screen.rerender(<WorkflowBoardPage />);

    const unhandledRejections: unknown[] = [];
    const onUnhandled = (event: PromiseRejectionEvent) => {
      unhandledRejections.push(event.reason);
    };
    window.addEventListener("unhandledrejection", onUnhandled);
    using _unhandled = {
      [Symbol.dispose]: () => window.removeEventListener("unhandledrejection", onUnhandled),
    };

    await screen.getByRole("button", { name: "Propose plan amendment" }).nth(1).click();
    await screen.getByRole("button", { name: "Submit amendment" }).click();
    const gitPathAlert = screen
      .getByRole("alert")
      .filter({ hasText: "Git metadata paths (.git) cannot be added to a task's allowed files" });
    await expect.element(gitPathAlert).toBeVisible();
    const amendmentReasonInput = screen.container.querySelector("#workflow-amendment-reason")!;
    expect(amendmentReasonInput.getAttribute("aria-invalid")).toBe("true");
    expect(amendmentReasonInput.getAttribute("aria-describedby")).toBeTruthy();
    expect(unhandledRejections).toEqual([]);
    expect(state.calls.some(([label]) => label === "workflow board propose amendment")).toBe(false);

    await screen.getByRole("checkbox").click();
    await expect.element(gitPathAlert).not.toBeInTheDocument();

    const amendDeferred = Promise.withResolvers<{
      readonly _tag: "Success";
      readonly value: unknown;
    }>();
    state.proposeAmendmentHandler = () => amendDeferred.promise;
    await screen.getByRole("button", { name: "Submit amendment" }).click();
    await expect.element(screen.getByRole("checkbox")).toBeDisabled();
    amendDeferred.resolve({
      _tag: "Success",
      value: {
        cardId: CARD_1,
        revision: PositiveInt.make(7),
        stage: "plan",
        amendment: {
          id: "amd-1",
          cardRevision: PositiveInt.make(7),
          targetStage: "plan",
          status: "applied",
          reason: "Updated scope",
          summary: null,
          sourceTaskId: "TSK-01",
          unexpectedFiles: [],
          createdAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
        },
        specApproval: null,
        planApproval: null,
        card: makeDetail({ revision: PositiveInt.make(7), stage: "plan" }),
      },
    });
    await vi.waitFor(() => {
      const amendCall = state.calls.find(([label]) => label === "workflow board propose amendment");
      expect(amendCall).toBeDefined();
      expect(amendCall![1]).toMatchObject({
        targetStage: "plan",
        sourceTaskId: "TSK-01",
        unexpectedFiles: [".git/config"],
      });
      expect("tasks" in (amendCall![1] as Record<string, unknown>)).toBe(false);
    });

    // Clean Execute card allows launching DAG-ready TSK-02 and fresh-thread review (no stableLaunchKey)
    state.calls = [];
    state.cardDetail = {
      ...executeDetail,
      status: "active",
      blockerSummary: null,
      fileContractReports: [],
      reviewReport: null,
    };
    await screen.rerender(<WorkflowBoardPage />);

    const task2El = screen.container.querySelector('[data-execute-task-id="TSK-02"]')!;
    const startTask2Btn = task2El.querySelector("button")!;
    startTask2Btn.click();
    await vi.waitFor(() => {
      const startCall = state.calls.find(([label]) => label === "workflow board start stage");
      expect(startCall).toBeDefined();
      expect(startCall![1]).toMatchObject({
        stage: "execute",
        taskId: "TSK-02",
      });
    });

    state.calls = [];
    await screen.getByRole("button", { name: "Start independent review" }).click();
    await vi.waitFor(() => {
      const reviewCall = state.calls.find(([label]) => label === "workflow board start stage");
      expect(reviewCall).toBeDefined();
      expect(reviewCall![1]).toMatchObject({ stage: "review" });
      expect("stableLaunchKey" in (reviewCall![1] as Record<string, unknown>)).toBe(false);
    });

    // Paused card needing reconciliation invokes resumeCard({ reconcileInterrupted: true })
    state.calls = [];
    state.cardDetail = {
      ...executeDetail,
      status: "paused",
      incompleteJournalCount: 1,
      recoveryState: {
        incompleteJournalCount: 1,
        pendingJournalEntries: [
          {
            id: "j-1",
            targetPath: WorkflowBoardArtifactKey.make("plan.md"),
            operation: "write",
            status: "file_written",
            expectedDigest: "a",
            previousDigest: null,
            revision: PositiveInt.make(5),
            lastError: null,
            createdAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
            updatedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
            completedAt: null,
          },
        ],
        externalEdits: [],
        requiresReconciliation: true,
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByText("Reconciliation required")).toBeVisible();
    await screen.getByRole("button", { name: "Reconcile & resume" }).first().click();
    await vi.waitFor(() => {
      const resumeCall = state.calls.find(([label]) => label === "workflow board resume card");
      expect(resumeCall).toBeDefined();
      expect(resumeCall![1]).toMatchObject({ reconcileInterrupted: true });
    });
  });

  it("supports M3 Ship diff, Go to fix pointer navigation, manual acceptance, DeliveryReceipt, and discrete Git actions", async () => {
    const blockedShipDetail = makeDetail({
      revision: PositiveInt.make(8),
      stage: "ship",
      status: "blocked",
      blockerSummary: "AC-1 requires verification",
      deliveryStatus: "blocked",
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      planApprovedRevision: PositiveInt.make(4),
      planApprovalStatus: "valid",
      shipReadiness: {
        status: "blocked",
        ready: false,
        productSnapshotDigest: "sha256:snap-1",
        blockers: [
          {
            id: "blk-ac",
            kind: "ac_unverified",
            message: "Acceptance criterion AC-1 is pending",
            pointer: { stage: "ship", tab: "acceptance", targetId: "AC-1" },
            blocking: true,
          },
          {
            id: "blk-chk",
            kind: "check_stale",
            message: "Planned check CHK-01 is stale",
            pointer: { stage: "execute", tab: "checks", targetId: "CHK-01" },
            blocking: true,
          },
        ],
        deliveryReceipt: null,
      },
      checkReceipts: [
        {
          checkId: "CHK-01",
          criterionId: "AC-1",
          ownerTaskId: "TSK-01",
          argv: ["node", "-e", "process.exit(0)"],
          status: "stale",
          exitCode: 0,
          logRef: null,
          productSnapshotDigest: "sha256:snap-0",
          executedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-24T00:04:00.000Z"),
        },
      ],
    });

    const cardDiff: WorkflowBoardGetCardDiffResult = {
      cardId: CARD_1,
      worktreePath: "/repo/.worktrees/card-1",
      baseRef: "main",
      productSnapshotDigest: "sha256:snap-1",
      diffDigest: "sha256:diff-1",
      files: [
        {
          path: WorkflowBoardArtifactKey.make("src/core.ts"),
          previousPath: null,
          changeType: "change",
          additions: NonNegativeInt.make(3),
          deletions: NonNegativeInt.make(0),
          patch:
            "diff --git a/src/core.ts b/src/core.ts\n--- a/src/core.ts\n+++ b/src/core.ts\n@@ -1 +1 @@\n-old\n+new\n",
          contractStatus: "allowed",
        },
      ],
      truncated: false,
      generatedAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
    };

    const vcsStatus: VcsStatusResult = {
      isRepo: true,
      hasPrimaryRemote: true,
      isDefaultRef: false,
      refName: "feat/card-1",
      hasWorkingTreeChanges: true,
      workingTree: {
        rootPath: "/repo/.worktrees/card-1",
        files: [
          { path: "src/core.ts", insertions: 3, deletions: 0 },
          { path: ".legion/workflows/card-1/spec.md", insertions: 10, deletions: 0 },
        ],
        insertions: 13,
        deletions: 0,
      },
      hasUpstream: true,
      aheadCount: 1,
      behindCount: 0,
      pr: null,
    };

    setup({
      search: { stage: "ship", card: CARD_1 },
      snapshot: {
        projectId: PROJECT_ID,
        revision: NonNegativeInt.make(8),
        cards: [
          makeSummary({
            cardId: CARD_1,
            title: "Feature A",
            stage: "ship",
            status: "blocked",
            blockerSummary: "AC-1 requires verification",
            deliveryStatus: "blocked",
            specApprovalStatus: "valid",
            planApprovalStatus: "valid",
          }),
        ],
        generatedAt: IsoDateTime.make("2026-11-24T00:05:00.000Z"),
      },
      cardDetail: blockedShipDetail,
      cardDiff,
      vcsStatus,
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    // Diff viewer renders card file & contract status
    await expect.element(screen.getByTestId("mock-styled-diff")).toBeVisible();
    await expect.element(screen.getByText("Allowed")).toBeVisible();

    // Approve delivery is disabled while blocked
    await expect.element(screen.getByRole("button", { name: "Approve delivery" })).toBeDisabled();

    // Go to fix pointer jumps to Execute Checks tab, highlights CHK-01, and moves focus to it
    const goToFixButton = screen.getByRole("button", { name: "Go to fix" }).nth(1);
    await expect
      .element(goToFixButton)
      .toHaveAttribute("aria-describedby", "ship-blocker-msg-blk-chk");
    await goToFixButton.click();
    await expect
      .element(screen.getByRole("heading", { name: "Planned checks & receipts" }))
      .toBeVisible();
    const checkTarget = screen.container.querySelector('[data-check-id="CHK-01"]');
    expect(checkTarget?.className ?? "").toContain("border-primary");
    await vi.waitFor(() => {
      expect(document.activeElement).toBe(checkTarget);
    });

    // Return to Ship tab & validate manual acceptance (fail requires note)
    await screen.getByRole("tab", { name: "Ship" }).click();
    await screen.getByRole("combobox", { name: "Verdict for AC-1" }).click();
    await browserPage.getByRole("option", { name: "Fail" }).click();
    await screen.getByRole("button", { name: "Record observation" }).click();
    await expect
      .element(screen.getByText("A note is required when recording Fail or Not applicable."))
      .toBeVisible();

    await screen.getByRole("combobox", { name: "Verdict for AC-1" }).click();
    await browserPage.getByRole("option", { name: "Pass" }).click();
    await screen.getByLabelText("Observation note for AC-1").fill("Verified in Chromium");
    await screen.getByRole("button", { name: "Record observation" }).click();
    await vi.waitFor(() => {
      const acCall = state.calls.find(
        ([label]) => label === "workflow board record manual acceptance",
      );
      expect(acCall).toBeDefined();
      expect(acCall![1]).toMatchObject({
        criterionId: "AC-1",
        verdict: "pass",
        note: "Verified in Chromium",
      });
    });

    // Ready card enables Approve delivery and transitions to Shipped + DeliveryReceipt
    state.calls = [];
    state.cardDetail = {
      ...blockedShipDetail,
      status: "active",
      blockerSummary: null,
      deliveryStatus: "ready",
      shipReadiness: {
        status: "ready",
        ready: true,
        productSnapshotDigest: "sha256:snap-1",
        blockers: [],
        deliveryReceipt: null,
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByRole("button", { name: "Approve delivery" })).toBeEnabled();
    await screen.getByLabelText("Delivery approval note").fill("Ship it");
    await screen.getByRole("button", { name: "Approve delivery" }).click();
    await vi.waitFor(() => {
      const shipCall = state.calls.find(([label]) => label === "workflow board approve delivery");
      expect(shipCall).toBeDefined();
      expect(shipCall![1]).toMatchObject({
        expectedProductSnapshotDigest: "sha256:snap-1",
        note: "Ship it",
      });
    });

    // Discrete GitActionsControl commits ONLY card implementation files (no auto-commit, no stacked quick action)
    await screen.getByRole("button", { name: "Commit" }).click();
    await vi.waitFor(() => {
      const gitCall = state.calls.find(([label]) => label === "git run stacked action");
      expect(gitCall).toBeDefined();
      expect(gitCall![1]).toMatchObject({
        action: "commit",
        filePaths: ["src/core.ts"],
      });
    });

    // Discrete Push & Create PR via dropdown menu
    state.calls = [];
    await screen.getByRole("button", { name: "Git action options" }).click();
    await browserPage.getByRole("menuitem", { name: "Push" }).click();
    await vi.waitFor(() => {
      const pushCall = state.calls.find(([label]) => label === "git run stacked action");
      expect(pushCall).toBeDefined();
      expect(pushCall![1]).toMatchObject({ action: "push" });
    });

    // Shipped card remains visible in Ship sidebar & renders DeliveryReceipt
    state.search = { stage: "ship", card: CARD_1 };
    state.snapshot = {
      projectId: PROJECT_ID,
      revision: NonNegativeInt.make(10),
      cards: [
        makeSummary({
          cardId: CARD_1,
          title: "Feature A",
          stage: "ship",
          status: "completed",
          deliveryStatus: "shipped",
        }),
      ],
      generatedAt: IsoDateTime.make("2026-11-24T00:08:00.000Z"),
    };
    state.cardDetail = {
      ...blockedShipDetail,
      revision: PositiveInt.make(10),
      status: "completed",
      deliveryStatus: "shipped",
      deliveryReceipt: {
        id: "del-1",
        approvalId: WorkflowBoardApprovalId.make("appr-del-1"),
        cardRevision: PositiveInt.make(10),
        productSnapshotDigest: "sha256:snap-1",
        commitSha: null,
        specDigest: "sha256:digest-1",
        planDigest: "sha256:plan-1",
        tasksDigest: "sha256:tasks-1",
        checksDigest: "sha256:checks-1",
        reviewId: "rev-1",
        acVerdictsSummary: "1/1 pass",
        approvedBy: "client",
        approvedAt: IsoDateTime.make("2026-11-24T00:08:00.000Z"),
        status: "shipped",
      },
      shipReadiness: {
        status: "shipped",
        ready: true,
        productSnapshotDigest: "sha256:snap-1",
        blockers: [],
        deliveryReceipt: null,
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByRole("heading", { name: "Delivery receipt" })).toBeVisible();
    await expect.element(screen.getByText("del-1")).toBeVisible();
    await expect.element(screen.getByRole("button", { name: /Feature A/ })).toBeVisible();

    // Overview execute -> ship Alt+ArrowRight transition dialog surfaces blockers and Inspect Ship blockers jumps to Ship
    state.search = {};
    state.calls = [];
    state.approveDeliveryError = new Error("Ship blocker remains");
    state.snapshot = {
      projectId: PROJECT_ID,
      revision: NonNegativeInt.make(11),
      cards: [
        makeSummary({
          cardId: CARD_1,
          title: "Feature A",
          stage: "execute",
          status: "blocked",
          blockerSummary: "Ship blocker remains",
          specApprovalStatus: "valid",
          planApprovalStatus: "valid",
        }),
      ],
      generatedAt: IsoDateTime.make("2026-11-24T00:09:00.000Z"),
    };
    state.cardDetail = blockedShipDetail;
    await screen.rerender(<WorkflowBoardPage />);
    const execCardButton = screen.container.querySelector<HTMLElement>(
      `[data-board-card-id="${CARD_1}"] button:not([data-board-drag-handle])`,
    )!;
    execCardButton.focus();
    await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
    await expect.element(browserPage.getByText("Approve delivery and mark shipped")).toBeVisible();
    await browserPage.getByRole("button", { name: "Confirm delivery approval" }).click();
    await expect
      .element(browserPage.getByRole("button", { name: "Inspect Ship blockers" }))
      .toBeVisible();
    await browserPage.getByRole("button", { name: "Inspect Ship blockers" }).click();
    expect(state.search.stage).toBe("ship");
    expect(state.search.card).toBe(CARD_1);
  });

  it("previews CLI import with active lock refusal, applies with preserved/disambiguated ID, and gates Execute on policy ack and blocked TSK-0002", async () => {
    const importedCardId = WorkflowCardId.make("cli-spec-t3-code-1");
    const blockingPolicy = {
      key: "sandbox.requireHardened",
      category: "sandbox",
      summary: "Requires hardened sandbox enforcement",
      configuredValue: "true",
      status: "unsupported" as const,
      blocksExecution: true,
      sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
    };
    const missingLogNote = {
      taskId: "TSK-0002",
      checkId: "chk-cli-exec-1",
      relativePath: WorkflowBoardArtifactKey.make(
        ".legion-cli/cache/runs/execute-mviwmzlq-24861406/verify-1.log",
      ),
      command: "node --test packages/provider-mimo/qualification/cli.test.mjs",
      note: "Gitignored cache log missing on disk; verification command failed with exit 1.",
    };
    const histReceipt = {
      id: "hist-spec-approval",
      kind: "spec_approval" as const,
      status: "historical" as const,
      sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/workflow/spec-approval.yaml"),
      recordedAt: "2026-10-22T01:00:17.476Z",
      recordedBy: "user",
      summary: "Approved in CLI",
      requiresNativeRevalidation: true,
    };

    const importedDetail = makeDetail({
      cardId: importedCardId,
      title: "T3 Code",
      stage: "execute",
      status: "blocked",
      blockerSummary: "verification command failed with exit 1",
      nextAction: "acknowledge_policies",
      specApprovalStatus: "historical",
      planApprovalStatus: "historical",
      tasks: [
        {
          taskId: "TSK-0001",
          title: "Ready task",
          filesAllowed: [WorkflowBoardNonGitArtifactKey.make("apps/mobile/vite.config.ts")],
          testingMethod: "test-first",
          status: "todo",
        },
        {
          taskId: "TSK-0002",
          title: "Provider mimo qualification",
          filesAllowed: [
            WorkflowBoardNonGitArtifactKey.make(
              "packages/provider-mimo/qualification/cli.test.mjs",
            ),
          ],
          testingMethod: "test-first",
          status: "blocked",
        },
      ],
      checkReceipts: [
        {
          checkId: "chk-cli-exec-1",
          argv: ["node", "--test", "packages/provider-mimo/qualification/cli.test.mjs"],
          status: "historical",
          exitCode: 1,
          logRef: null,
          productSnapshotDigest: null,
          executedAt: null,
          updatedAt: IsoDateTime.make("2026-10-22T02:28:41.128Z"),
          staleReason: "imported_historical_cli_receipt",
        },
      ],
      reviewReport: {
        reviewId: "rev-cli-hist",
        stageRunId: "sr-cli-hist",
        threadId: null,
        status: "historical",
        verdict: "stale",
        summary: "Imported historical review",
        productSnapshotDigest: null,
        findings: [],
        createdAt: IsoDateTime.make("2026-10-22T02:28:41.128Z"),
        updatedAt: IsoDateTime.make("2026-10-22T02:28:41.128Z"),
        completedAt: null,
      },
      cliImport: {
        sourceSystem: "legion-cli",
        sourceCliDir: WorkflowBoardArtifactKey.make(".legion-cli"),
        sourceSpecId: "spec-t3-code",
        owner: "legioncode",
        importedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        lastSyncedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        sourceDigest: "sha256:cli-1",
        decisionIds: ["D-001", "D-002"],
        assumptionIds: ["ASM-0001"],
        acceptanceCriterionIds: ["AC-P0-01"],
        historicalReceipts: [histReceipt],
        unsupportedPolicies: [blockingPolicy],
        acknowledgedPolicies: [],
        missingLogNotes: [missingLogNote],
        preservedArtifactPaths: [WorkflowBoardArtifactKey.make("spec.md")],
        assurancePolicySummary: null,
      },
    });

    // 1. Active CLI run lock refusal disables Apply import
    setup({
      previewCliImportResult: {
        projectId: PROJECT_ID,
        sourcePath: WorkflowBoardArtifactKey.make(".legion-cli"),
        specId: "spec-t3-code",
        title: "T3 Code",
        stage: "execute",
        status: "blocked",
        existingCardId: null,
        alreadyImported: false,
        activeLocks: [
          {
            kind: "engine_lock",
            relativePath: WorkflowBoardArtifactKey.make(".legion-cli/index/engine.lock"),
            status: "active",
            pid: PositiveInt.make(4242),
            runId: "run-active-1",
            startedAt: null,
            message: "CLI engine lock active",
          },
        ],
        staleLocks: [],
        canApply: false,
        refusalReason: "Active CLI run lock conflict detected; refusing takeover",
        tasks: [],
        decisionIds: ["D-001"],
        assumptionIds: ["ASM-0001"],
        acceptanceCriterionIds: ["AC-P0-01"],
        historicalReceipts: [histReceipt],
        unsupportedPolicies: [blockingPolicy],
        missingLogNotes: [missingLogNote],
        warnings: [],
      },
    });
    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    await screen.getByRole("button", { name: "Import CLI project" }).click();
    await expect
      .element(browserPage.getByText("Active CLI run detected: takeover refused"))
      .toBeVisible();
    await expect
      .element(browserPage.getByText("Active CLI run lock conflict detected; refusing takeover"))
      .toBeVisible();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeDisabled();

    // Unsafe path validation
    await browserPage.getByLabelText("CLI directory (relative to workspace)").fill("../escape");
    await expect
      .element(
        browserPage.getByText(
          "Enter a safe workspace-relative path (no traversal, .git, or credential files).",
        ),
      )
      .toBeVisible();
    await expect
      .element(browserPage.getByRole("button", { name: "Preview import" }))
      .toBeDisabled();

    // 2. Clean preview with stale lock, blocked TSK-0002, unsupported policies, and Apply -> selects returned cardId
    state.previewCliImportResult = {
      projectId: PROJECT_ID,
      sourcePath: WorkflowBoardArtifactKey.make(".legion-cli"),
      specId: "spec-t3-code",
      title: "T3 Code",
      stage: "execute",
      status: "blocked",
      existingCardId: null,
      alreadyImported: false,
      activeLocks: [],
      staleLocks: [
        {
          kind: "live_spawn",
          relativePath: WorkflowBoardArtifactKey.make(".legion-cli/cache/live-spawn/dead.json"),
          status: "stale",
          pid: PositiveInt.make(999999),
          runId: "dead-1",
          startedAt: null,
          message: "Stale PID ignored",
        },
      ],
      canApply: true,
      refusalReason: null,
      tasks: [
        {
          id: "t-1",
          taskId: "TSK-0001",
          title: "Ready task",
          status: "todo",
          blockerSummary: null,
          verificationCommands: [],
          missingLogNotes: [],
        },
        {
          id: "t-2",
          taskId: "TSK-0002",
          title: "Provider mimo qualification",
          status: "blocked",
          blockerSummary: "verification command failed with exit 1",
          verificationCommands: [
            ["node", "--test", "packages/provider-mimo/qualification/cli.test.mjs"],
          ],
          missingLogNotes: [missingLogNote],
        },
      ],
      decisionIds: ["D-001", "D-002"],
      assumptionIds: ["ASM-0001"],
      acceptanceCriterionIds: ["AC-P0-01"],
      historicalReceipts: [histReceipt],
      unsupportedPolicies: [blockingPolicy],
      missingLogNotes: [missingLogNote],
      warnings: [],
    };
    state.applyCliImportResult = {
      cardId: importedCardId,
      projectId: PROJECT_ID,
      revision: PositiveInt.make(1),
      created: true,
      sourceSpecId: "spec-t3-code",
      tasks: importedDetail.tasks!,
      historicalReceipts: [histReceipt],
      unsupportedPolicies: [blockingPolicy],
      missingLogNotes: [missingLogNote],
      card: importedDetail,
    };

    await browserPage.getByLabelText("CLI directory (relative to workspace)").fill(".legion-cli");
    await browserPage.getByRole("button", { name: "Preview import" }).click();
    await expect.element(browserPage.getByText("Stale CLI locks ignored")).toBeVisible();
    await expect.element(browserPage.getByText("sandbox.requireHardened")).toBeVisible();
    await expect.element(browserPage.getByText("TSK-0002")).toBeVisible();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeEnabled();

    await browserPage.getByRole("button", { name: "Apply import" }).click();
    await vi.waitFor(() => {
      expect(state.calls.some(([label]) => label === "workflow board apply cli import")).toBe(true);
      expect(state.search.card).toBe(importedCardId);
      expect(state.search.stage).toBe("execute");
    });

    // 3. Render imported card workspace: provenance badges, Execute policy ack gate, blocked TSK-0002, and Review Historical badge
    state.snapshot = {
      projectId: PROJECT_ID,
      revision: NonNegativeInt.make(2),
      cards: [
        makeSummary({
          cardId: importedCardId,
          title: "T3 Code",
          stage: "execute",
          status: "blocked",
          blockerSummary: "verification command failed with exit 1",
          nextAction: "acknowledge_policies",
          specApprovalStatus: "historical",
          planApprovalStatus: "historical",
        }),
      ],
      generatedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
    };
    state.cardDetail = importedDetail;
    state.calls = [];
    await screen.rerender(<WorkflowBoardPage />);

    await expect.element(screen.getByText("Imported from .legion-cli")).toBeVisible();
    await expect.element(screen.getByText("Managed by LegionCode")).toBeVisible();
    await expect.element(screen.getByText("CLI import provenance (spec-t3-code)")).toBeVisible();
    await expect.element(screen.getByText("Unsupported .legion-cli policies (1)")).toBeVisible();

    const task1Row = screen.container.querySelector('[data-execute-task-id="TSK-0001"]')!;
    const task2Row = screen.container.querySelector('[data-execute-task-id="TSK-0002"]')!;
    expect(task1Row.querySelector("button:last-of-type")?.hasAttribute("disabled")).toBe(true);
    expect(task2Row.textContent).toContain("verify-1.log");

    // Acknowledge unsupported policies -> TSK-0001 Start task becomes enabled while TSK-0002 remains blocked until Retry task
    await screen
      .getByRole("checkbox", {
        name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
      })
      .click();
    expect(task1Row.querySelector("button:last-of-type")?.hasAttribute("disabled")).toBe(false);
    expect(task2Row.querySelector("button:last-of-type")?.hasAttribute("disabled")).toBe(true);

    await (task1Row.querySelector("button:last-of-type") as HTMLButtonElement).click();
    await vi.waitFor(() => {
      const startCall = state.calls.find(([label]) => label === "workflow board start stage");
      expect(startCall).toBeDefined();
      expect(startCall![1]).toMatchObject({
        cardId: importedCardId,
        stage: "execute",
        taskId: "TSK-0001",
        acknowledgeUnsupportedPolicies: true,
        acknowledgedPolicies: ["sandbox.requireHardened"],
      });
    });

    // Review panel renders Historical badge alongside Stale
    const reviewHeading = screen.getByRole("heading", { name: "Independent review" });
    await expect.element(reviewHeading).toBeVisible();
    expect(
      screen.container.querySelector('[aria-labelledby="execute-review-heading"]')?.textContent,
    ).toContain("Historical");

    // Case B re-import introducing a new blocking policy resets local policiesAcknowledged checkbox
    state.cardDetail = {
      ...importedDetail,
      revision: PositiveInt.make(2),
      cliImport: {
        ...importedDetail.cliImport!,
        unsupportedPolicies: [
          blockingPolicy,
          {
            key: "qa.passScore",
            category: "qa",
            summary: "Requires minimum QA pass score",
            configuredValue: "90",
            status: "unsupported",
            blocksExecution: true,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
        ],
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await expect.element(screen.getByText("Unsupported .legion-cli policies (2)")).toBeVisible();
    await expect
      .element(
        screen.getByRole("checkbox", {
          name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
        }),
      )
      .not.toBeChecked();
    expect(task1Row.querySelector("button:last-of-type")?.hasAttribute("disabled")).toBe(true);

    // [A] -> [] -> [A] policy transition resets prior acknowledgement so re-introduced policy A requires fresh ack
    await screen
      .getByRole("checkbox", {
        name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
      })
      .click();
    await expect
      .element(
        screen.getByRole("checkbox", {
          name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
        }),
      )
      .toBeChecked();

    state.cardDetail = {
      ...importedDetail,
      revision: PositiveInt.make(3),
      cliImport: {
        ...importedDetail.cliImport!,
        unsupportedPolicies: [],
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    state.cardDetail = {
      ...importedDetail,
      revision: PositiveInt.make(4),
      cliImport: {
        ...importedDetail.cliImport!,
        unsupportedPolicies: [blockingPolicy],
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(
        screen.getByRole("checkbox", {
          name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
        }),
      )
      .not.toBeChecked();
    expect(task1Row.querySelector("button:last-of-type")?.hasAttribute("disabled")).toBe(true);

    // [A] -> [B] -> [A] without passing through [] or changing timestamps also resets prior acknowledgement
    await screen
      .getByRole("checkbox", {
        name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
      })
      .click();
    await expect
      .element(
        screen.getByRole("checkbox", {
          name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
        }),
      )
      .toBeChecked();

    state.cardDetail = {
      ...importedDetail,
      revision: PositiveInt.make(5),
      cliImport: {
        ...importedDetail.cliImport!,
        unsupportedPolicies: [
          {
            key: "qa.passScore",
            category: "qa",
            summary: "Requires minimum QA pass score",
            configuredValue: "90",
            status: "unsupported",
            blocksExecution: true,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
        ],
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    state.cardDetail = {
      ...importedDetail,
      revision: PositiveInt.make(6),
      cliImport: {
        ...importedDetail.cliImport!,
        unsupportedPolicies: [blockingPolicy],
      },
    };
    await screen.rerender(<WorkflowBoardPage />);
    await expect
      .element(
        screen.getByRole("checkbox", {
          name: "I acknowledge these unsupported .legion-cli policies and want to run Execute in LegionCode",
        }),
      )
      .not.toBeChecked();
    expect(task1Row.querySelector("button:last-of-type")?.hasAttribute("disabled")).toBe(true);
  });

  it("invalidates stale CLI import preview when cliDir is edited and drops out-of-order initial preview responses", async () => {
    const makePreview = (cliDir: string, title: string): WorkflowBoardPreviewCliImportResult => ({
      projectId: PROJECT_ID,
      sourcePath: WorkflowBoardArtifactKey.make(cliDir),
      specId: `spec-${title.toLowerCase().replaceAll(/\s+/g, "-")}`,
      title,
      stage: "execute",
      status: "active",
      existingCardId: null,
      alreadyImported: false,
      activeLocks: [],
      staleLocks: [],
      canApply: true,
      refusalReason: null,
      tasks: [],
      decisionIds: [],
      assumptionIds: [],
      acceptanceCriterionIds: [],
      historicalReceipts: [],
      unsupportedPolicies: [],
      missingLogNotes: [],
      warnings: [],
    });

    const initialDeferred = Promise.withResolvers<{
      readonly _tag: "Success";
      readonly value: WorkflowBoardPreviewCliImportResult;
    }>();
    let reopenCount = 0;

    setup({
      previewCliImportHandler: async (input) => {
        const dir = input.cliDir ?? ".legion-cli";
        if (dir === ".legion-cli") {
          reopenCount += 1;
          if (reopenCount === 1) {
            return initialDeferred.promise;
          }
          return {
            _tag: "Success",
            value: makePreview(".legion-cli", "Reopened Default CLI"),
          };
        }
        if (dir === ".legion-cli-alt") {
          return {
            _tag: "Success",
            value: makePreview(".legion-cli-alt", "Custom Alt CLI"),
          };
        }
        return {
          _tag: "Success",
          value: makePreview(dir, "Other CLI"),
        };
      },
    });

    await using _desktop = await desktop();
    await using screen = await render(<WorkflowBoardPage />);

    // 1. Open dialog: initial .legion-cli preview is in-flight -> loading state shown, Apply disabled
    await screen.getByRole("button", { name: "Import CLI project" }).click();
    await expect.element(browserPage.getByRole("button", { name: "Previewing…" })).toBeDisabled();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeDisabled();

    // 2. Edit cliDir to custom path and click Preview import while initial preview is still pending
    await browserPage
      .getByLabelText("CLI directory (relative to workspace)")
      .fill(".legion-cli-alt");
    await expect.element(browserPage.getByRole("button", { name: "Preview import" })).toBeEnabled();
    await browserPage.getByRole("button", { name: "Preview import" }).click();
    await expect.element(browserPage.getByText("Custom Alt CLI")).toBeVisible();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeEnabled();

    // 3. Slow initial .legion-cli preview resolves AFTER custom preview -> must NOT overwrite custom preview
    initialDeferred.resolve({
      _tag: "Success",
      value: makePreview(".legion-cli", "Slow Default CLI"),
    });
    await initialDeferred.promise;
    await expect.element(browserPage.getByText("Custom Alt CLI")).toBeVisible();
    await expect.element(browserPage.getByText("Slow Default CLI")).not.toBeInTheDocument();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeEnabled();

    // 4. Editing cliDir after preview invalidates stale preview and disables Apply import until re-previewed
    await browserPage
      .getByLabelText("CLI directory (relative to workspace)")
      .fill(".legion-cli-other");
    await expect.element(browserPage.getByText("Custom Alt CLI")).not.toBeInTheDocument();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeDisabled();

    await browserPage.getByRole("button", { name: "Preview import" }).click();
    await expect.element(browserPage.getByText("Other CLI")).toBeVisible();
    await expect.element(browserPage.getByRole("button", { name: "Apply import" })).toBeEnabled();

    // 5. Closing and reopening resets cliDir to ".legion-cli" and previews default directory cleanly
    await browserPage.getByRole("button", { name: "Cancel" }).click();
    await screen.getByRole("button", { name: "Import CLI project" }).click();
    await expect
      .element(browserPage.getByLabelText("CLI directory (relative to workspace)"))
      .toHaveValue(".legion-cli");
    await expect.element(browserPage.getByText("Reopened Default CLI")).toBeVisible();

    // 6. While applying is true: Cancel is disabled, Escape does not close, failure keeps dialog open and re-enables Cancel, success closes cleanly
    const applyFailDeferred = Promise.withResolvers<{
      readonly _tag: "Failure";
      readonly cause: Cause.Cause<unknown>;
    }>();
    state.applyCliImportHandler = () => applyFailDeferred.promise;
    await browserPage.getByRole("button", { name: "Apply import" }).click();
    await expect.element(browserPage.getByRole("button", { name: "Importing…" })).toBeDisabled();
    await expect.element(browserPage.getByRole("button", { name: "Cancel" })).toBeDisabled();
    await userEvent.keyboard("{Escape}");
    await expect.element(browserPage.getByRole("button", { name: "Importing…" })).toBeVisible();

    applyFailDeferred.resolve({
      _tag: "Failure",
      cause: Cause.fail(new Error("Import failed on server")),
    });
    await expect.element(browserPage.getByText("Import failed on server")).toBeVisible();
    await expect.element(browserPage.getByRole("button", { name: "Cancel" })).toBeEnabled();

    const importedCardId = WorkflowCardId.make("cli-spec-reopened");
    state.applyCliImportHandler = () =>
      Promise.resolve({
        _tag: "Success",
        value: {
          cardId: importedCardId,
          projectId: PROJECT_ID,
          revision: PositiveInt.make(1),
          created: true,
          sourceSpecId: "spec-reopened",
          tasks: [],
          historicalReceipts: [],
          unsupportedPolicies: [],
          missingLogNotes: [],
          card: makeDetail({ cardId: importedCardId, stage: "execute" }),
        },
      });
    await browserPage.getByRole("button", { name: "Apply import" }).click();
    await vi.waitFor(() => {
      expect(state.search.card).toBe(importedCardId);
    });
    await expect
      .element(browserPage.getByRole("heading", { name: "Import CLI project" }))
      .not.toBeInTheDocument();
  });
});

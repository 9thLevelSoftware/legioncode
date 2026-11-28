import {
  DEFAULT_SERVER_SETTINGS,
  EnvironmentId,
  IsoDateTime,
  PositiveInt,
  ProjectId,
  RunId,
  ThreadId,
  WorkflowBoardStageRunId,
  WorkflowCardId,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { describe, expect, it } from "vite-plus/test";

import {
  collectMobileCardThreadLinks,
  defaultMobileBoardStageFilter,
  defaultMobileCardSubTab,
  groupWorkflowBoardCards,
  mobileCardThreadLinkLabel,
  mobileFindingDispositionLabel,
  mobileManualAcceptanceVerdictLabel,
  mobilePlanApprovalBlockerMessage,
  resolveMobileWorkflowBoardTarget,
  selectMobileBoardStageCards,
  workflowCardStateKey,
  type MobileEnvironmentInput,
  type MobileProjectInput,
} from "./workflowBoard.logic";

const ENV_1 = EnvironmentId.make("env-1");
const ENV_2 = EnvironmentId.make("env-2");
const PROJ_1 = ProjectId.make("proj-1");
const PROJ_2 = ProjectId.make("proj-2");

function makeEnvironment(
  environmentId: EnvironmentId,
  label: string,
  opts: {
    readonly phase: "connected" | "connecting" | "disconnected";
    readonly workflowsEnabled?: boolean;
    readonly projectWorkflowBoard?: boolean | undefined;
    readonly configLoaded?: boolean;
  },
): MobileEnvironmentInput {
  return {
    environmentId,
    label,
    connection: { phase: opts.phase } as MobileEnvironmentInput["connection"],
    serverConfig:
      opts.configLoaded === false
        ? null
        : ({
            settings: {
              ...DEFAULT_SERVER_SETTINGS,
              workflowsEnabled: opts.workflowsEnabled ?? false,
            },
            environment: {
              capabilities:
                opts.projectWorkflowBoard === undefined
                  ? {}
                  : { projectWorkflowBoard: opts.projectWorkflowBoard },
            },
          } as unknown as MobileEnvironmentInput["serverConfig"]),
  };
}

function makeProject(
  environmentId: EnvironmentId,
  id: ProjectId,
  title: string,
): MobileProjectInput {
  return {
    environmentId,
    id,
    title,
    workspaceRoot: `/repo/${title}`,
    archivedAt: null,
  };
}

describe("mobile workflowBoard.logic", () => {
  it("holds loading state during cold start and connecting phases before resolving capabilities", () => {
    const connectingEnv = makeEnvironment(ENV_1, "Laptop", {
      phase: "connecting",
      configLoaded: false,
    });
    const project = makeProject(ENV_1, PROJ_1, "t3code");

    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: false,
        environments: [connectingEnv],
        projects: [],
        requestedEnvironmentId: ENV_1,
        requestedProjectId: PROJ_1,
      }),
    ).toMatchObject({
      loading: true,
      availability: "loading",
      missingRequestedProject: false,
    });

    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [connectingEnv],
        projects: [project],
        requestedEnvironmentId: ENV_1,
        requestedProjectId: PROJ_1,
      }),
    ).toMatchObject({
      loading: true,
      availability: "loading",
      missingRequestedProject: false,
    });

    const unsupportedEnv = makeEnvironment(ENV_1, "Legacy", {
      phase: "connected",
      workflowsEnabled: true,
      projectWorkflowBoard: undefined,
    });
    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [unsupportedEnv],
        projects: [project],
        requestedEnvironmentId: ENV_1,
        requestedProjectId: PROJ_1,
      }),
    ).toMatchObject({
      loading: false,
      availability: "unsupported",
      missingRequestedProject: false,
    });

    const labsDisabledEnv = makeEnvironment(ENV_1, "Laptop", {
      phase: "connected",
      workflowsEnabled: false,
      projectWorkflowBoard: true,
    });
    const enabledEnv = makeEnvironment(ENV_2, "Workstation", {
      phase: "connected",
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    const picked = resolveMobileWorkflowBoardTarget({
      isReady: true,
      environments: [labsDisabledEnv, enabledEnv],
      projects: [makeProject(ENV_1, PROJ_1, "Alpha"), makeProject(ENV_2, PROJ_2, "Beta")],
    });
    expect(picked.availability).toBe("enabled");
    expect(picked.selected?.environmentId).toBe(ENV_2);
    expect(picked.selected?.projectId).toBe(PROJ_2);
    expect(picked.candidates).toHaveLength(2);

    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [enabledEnv],
        projects: [makeProject(ENV_2, PROJ_2, "Beta")],
        requestedEnvironmentId: ENV_2,
        requestedProjectId: "missing-proj",
      }),
    ).toMatchObject({
      loading: false,
      selected: null,
      missingRequestedProject: true,
    });
  });

  it("reports true environment availability instead of Project not found when projects are empty", () => {
    const nullConfigConnectedEnv = makeEnvironment(ENV_1, "Cold", {
      phase: "connected",
      configLoaded: false,
    });
    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [nullConfigConnectedEnv],
        projects: [],
        requestedEnvironmentId: ENV_1,
        requestedProjectId: PROJ_1,
      }),
    ).toMatchObject({
      loading: true,
      selected: null,
      availability: "loading",
      missingRequestedProject: false,
    });

    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [nullConfigConnectedEnv],
        projects: [],
      }),
    ).toMatchObject({
      loading: true,
      selected: null,
      availability: "loading",
      missingRequestedProject: false,
    });

    const unsupportedEnv = makeEnvironment(ENV_1, "Legacy", {
      phase: "connected",
      workflowsEnabled: true,
      projectWorkflowBoard: undefined,
    });
    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [unsupportedEnv],
        projects: [],
      }),
    ).toMatchObject({
      loading: false,
      selected: null,
      availability: "unsupported",
      missingRequestedProject: false,
    });

    const labsDisabledEnv = makeEnvironment(ENV_1, "Laptop", {
      phase: "connected",
      workflowsEnabled: false,
      projectWorkflowBoard: true,
    });
    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [labsDisabledEnv],
        projects: [],
      }),
    ).toMatchObject({
      loading: false,
      selected: null,
      availability: "labs-disabled",
      missingRequestedProject: false,
    });

    const enabledEnv = makeEnvironment(ENV_1, "Laptop", {
      phase: "connected",
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [enabledEnv],
        projects: [],
      }),
    ).toMatchObject({
      loading: false,
      selected: null,
      availability: "enabled",
      missingRequestedProject: true,
    });

    const disconnectedEnv = makeEnvironment(ENV_1, "Offline", {
      phase: "disconnected",
      configLoaded: false,
    });
    expect(
      resolveMobileWorkflowBoardTarget({
        isReady: true,
        environments: [disconnectedEnv],
        projects: [],
      }),
    ).toMatchObject({
      loading: false,
      selected: null,
      availability: "disconnected",
      missingRequestedProject: false,
    });
  });

  it("groups stage cards, selects default stage/sub-tab, and deduplicates linked threads", () => {
    const columns = groupWorkflowBoardCards([
      {
        cardId: WorkflowCardId.make("c-exec"),
        projectId: PROJ_1,
        title: "Execute card",
        stage: "execute",
        status: "active",
        revision: PositiveInt.make(3),
        currentTask: "TSK-1",
        blockerSummary: null,
        nextAction: "complete_task",
        specApprovedRevision: PositiveInt.make(2),
        specApprovalStatus: "valid",
        activeThreadId: null,
        createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-27T01:00:00.000Z"),
      },
      {
        cardId: WorkflowCardId.make("c-done"),
        projectId: PROJ_1,
        title: "Done card",
        stage: "ship",
        status: "completed",
        revision: PositiveInt.make(5),
        currentTask: null,
        blockerSummary: null,
        nextAction: null,
        specApprovedRevision: PositiveInt.make(2),
        specApprovalStatus: "valid",
        activeThreadId: null,
        createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-27T02:00:00.000Z"),
      },
    ]);

    expect(defaultMobileBoardStageFilter(columns)).toBe("execute");
    expect(defaultMobileBoardStageFilter(columns, "completed")).toBe("completed");
    expect(selectMobileBoardStageCards(columns, "completed").map((c) => c.cardId)).toEqual([
      "c-done",
    ]);

    expect(defaultMobileCardSubTab({ stage: "plan", status: "active" })).toBe("plan");
    expect(defaultMobileCardSubTab({ stage: "ship", status: "completed" })).toBe("ship");
    expect(defaultMobileCardSubTab({ stage: "plan", status: "active" }, "history")).toBe("history");

    const tActive = ThreadId.make("t-active");
    const tPlan = ThreadId.make("t-plan");
    const tReview = ThreadId.make("t-review");
    const tTask = ThreadId.make("t-task");

    const links = collectMobileCardThreadLinks({
      activeThreadId: tActive,
      stageRuns: [
        {
          id: WorkflowBoardStageRunId.make("sr-1"),
          stage: "plan",
          stageRunId: undefined,
          stableLaunchKey: "k1",
          status: "completed",
          cardRevision: PositiveInt.make(2),
          runId: RunId.make("r-1"),
          threadId: tPlan,
          projectId: PROJ_1,
          workspaceStrategy: { type: "root" },
          worktreePath: null,
          branch: null,
          modelSelection: {
            provider: "codex",
            model: "gpt-5",
          } as unknown as WorkflowCardDetail["stageRuns"][number]["modelSelection"],
          runtimeMode: "full-access",
          interactionMode: "default",
          lastError: null,
          createdAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-27T00:01:00.000Z"),
          endedAt: null,
        } as unknown as WorkflowCardDetail["stageRuns"][number],
        {
          id: WorkflowBoardStageRunId.make("sr-2"),
          stage: "execute",
          stableLaunchKey: "k2",
          status: "running",
          cardRevision: PositiveInt.make(3),
          runId: RunId.make("r-2"),
          threadId: tActive, // duplicate of activeThreadId should deduplicate
          projectId: PROJ_1,
          workspaceStrategy: { type: "root" },
          worktreePath: null,
          branch: null,
          modelSelection: {
            provider: "codex",
            model: "gpt-5",
          } as unknown as WorkflowCardDetail["stageRuns"][number]["modelSelection"],
          runtimeMode: "full-access",
          interactionMode: "default",
          lastError: null,
          createdAt: IsoDateTime.make("2026-11-27T00:02:00.000Z"),
          updatedAt: IsoDateTime.make("2026-11-27T00:03:00.000Z"),
          endedAt: null,
        } as unknown as WorkflowCardDetail["stageRuns"][number],
      ],
      taskExecutions: [
        {
          taskId: "TSK-1",
          status: "in_progress",
          threadId: tTask,
          runId: null,
          stageRunId: null,
          worktreePath: null,
          branch: null,
          attempt: PositiveInt.make(1),
          startedAt: null,
          endedAt: null,
          lastError: null,
        },
      ],
      reviewReport: {
        reviewId: "rev-1",
        stageRunId: null,
        threadId: tReview,
        status: "completed",
        verdict: "findings",
        summary: "One finding",
        productSnapshotDigest: "snap-1",
        findings: [],
        createdAt: IsoDateTime.make("2026-11-27T00:04:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-27T00:04:00.000Z"),
        completedAt: IsoDateTime.make("2026-11-27T00:04:00.000Z"),
      },
    });

    expect(links.map((l) => l.threadId)).toEqual([tActive, tPlan, tReview, tTask]);
    expect(mobileCardThreadLinkLabel(links[0]!)).toBe("Open active thread");
    expect(mobileCardThreadLinkLabel(links[3]!)).toBe("Open task TSK-1 thread");

    expect(
      collectMobileCardThreadLinks({
        activeThreadId: "  " as ThreadId,
        stageRuns: [],
        taskExecutions: [
          {
            taskId: "TSK-blank",
            status: "failed",
            threadId: "" as ThreadId,
            runId: null,
            stageRunId: null,
            worktreePath: null,
            branch: null,
            attempt: PositiveInt.make(1),
            startedAt: null,
            endedAt: null,
            lastError: null,
          },
        ],
        reviewReport: null,
      }),
    ).toEqual([]);
  });

  it("formats blocker, finding, manual acceptance, and card state keys cleanly", () => {
    expect(mobilePlanApprovalBlockerMessage("ac_mappings_incomplete")).toContain("acceptance");
    expect(mobileFindingDispositionLabel("accept_risk")).toBe("Accept risk");
    expect(mobileManualAcceptanceVerdictLabel("not-applicable")).toBe("Not applicable");
    expect(workflowCardStateKey("env-1", "proj-1", "card-1")).toBe("env-1:proj-1:card-1");
  });
});

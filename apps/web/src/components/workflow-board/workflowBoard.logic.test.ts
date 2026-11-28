import type { EnvironmentServerConfig } from "@legioncode/client-runtime/connection";
import {
  DEFAULT_SERVER_SETTINGS,
  EnvironmentId,
  IsoDateTime,
  NonNegativeInt,
  PositiveInt,
  ProjectId,
  ProviderInstanceId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WorkflowBoardNonGitArtifactKey,
  WorkflowCardId,
  type ServerProvider,
  type WorkflowBoardTaskContract,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { describe, expect, it } from "vite-plus/test";

import type { SidebarProjectSnapshot } from "../../sidebarProjectGrouping";
import {
  buildAmendedTaskContractsWithUnexpectedFiles,
  buildWorkspaceStrategy,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  canStartExecuteTask,
  canStartIndependentReview,
  canStartWorkflowBoardPlan,
  collectDownstreamInvalidationStates,
  collectPlanApprovalBlockers,
  findInProgressTask,
  formatAcceptanceCriteriaLines,
  formatArgvTokens,
  isAcObservationStale,
  isCheckReceiptStale,
  isManualChallengeFallbackValid,
  isPlanOnlyEditPreservingSpecApproval,
  isPlanStrategyValid,
  isProjectGroupWorkflowBoardEnabled,
  isReviewReportStale,
  isSafeWorkflowBoardArgv,
  isSafeWorkflowBoardInputPath,
  isTaskDagReady,
  isWorkflowBoardWorkspaceStrategyValid,
  needsCardReconciliation,
  normalizeAcEvidenceMappings,
  normalizeWorkspaceStrategy,
  parseAcceptanceCriteriaLines,
  parseArgvTokens,
  parseSpecListLines,
  parseWorkflowBoardSearch,
  resolveCrossColumnTransition,
  resolveDefaultBoardLaunchConfig,
  resolveFixPointerNavigation,
  resolveWorkflowBoardProjectTarget,
  taskDependencyIds,
  unacknowledgedBlockingCliPolicies,
  validReturnTargetStages,
} from "./workflowBoard.logic";

const ENV_LOCAL = EnvironmentId.make("local");
const ENV_REMOTE = EnvironmentId.make("remote");
const PROJECT_1 = ProjectId.make("proj-1");
const PROJECT_2 = ProjectId.make("proj-2");

function makeConfig(overrides: {
  readonly workflowsEnabled: boolean;
  readonly projectWorkflowBoard?: boolean | undefined;
}): EnvironmentServerConfig {
  return {
    settings: {
      ...DEFAULT_SERVER_SETTINGS,
      workflowsEnabled: overrides.workflowsEnabled,
    },
    environment: {
      capabilities: {
        repositoryIdentity: true,
        ...(overrides.projectWorkflowBoard === undefined
          ? {}
          : { projectWorkflowBoard: overrides.projectWorkflowBoard }),
      },
    },
  } as unknown as EnvironmentServerConfig;
}

function makeGroup(): SidebarProjectSnapshot {
  return {
    id: PROJECT_1,
    environmentId: ENV_LOCAL,
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
    groupedProjectCount: 2,
    environmentPresence: "mixed",
    allRemoteMembersAreDesktopLocal: false,
    allRemoteMembersAreWsl: false,
    remoteEnvironmentLabels: ["Remote"],
    pinned: false,
    archived: false,
    memberProjectRefs: [
      { environmentId: ENV_LOCAL, projectId: PROJECT_1 },
      { environmentId: ENV_REMOTE, projectId: PROJECT_2 },
    ],
    memberProjects: [
      {
        id: PROJECT_1,
        environmentId: ENV_LOCAL,
        name: "t3code",
        workspaceRoot: "/repo/t3code",
        repositoryIdentity: null,
        defaultModelSelection: null,
        scripts: [],
        trackedFolders: [],
        createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
        isScratch: false,
        physicalProjectKey: "local:/repo/t3code",
        environmentLabel: "Local",
      },
      {
        id: PROJECT_2,
        environmentId: ENV_REMOTE,
        name: "t3code",
        workspaceRoot: "/srv/t3code",
        repositoryIdentity: null,
        defaultModelSelection: null,
        scripts: [],
        trackedFolders: [],
        createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
        updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
        isScratch: false,
        physicalProjectKey: "remote:/srv/t3code",
        environmentLabel: "Remote",
      },
    ],
  } as unknown as SidebarProjectSnapshot;
}

describe("workflowBoard.logic", () => {
  it("parses route search params safely", () => {
    expect(
      parseWorkflowBoardSearch({
        env: " local ",
        projectId: "proj-1",
        card: "card-1",
        stage: "plan",
      }),
    ).toEqual({
      env: "local",
      projectId: "proj-1",
      card: "card-1",
      stage: "plan",
    });

    expect(
      parseWorkflowBoardSearch({
        env: "  ",
        stage: "bogus",
      }),
    ).toEqual({});
  });

  it("gates project groups and resolves board target availability", () => {
    const group = makeGroup();
    const configs = new Map<EnvironmentId, EnvironmentServerConfig>([
      [ENV_LOCAL, makeConfig({ workflowsEnabled: false, projectWorkflowBoard: true })],
      [ENV_REMOTE, makeConfig({ workflowsEnabled: true, projectWorkflowBoard: true })],
    ]);

    expect(isProjectGroupWorkflowBoardEnabled(group, configs, [ENV_LOCAL])).toBe(false);
    expect(isProjectGroupWorkflowBoardEnabled(group, configs, [ENV_LOCAL, ENV_REMOTE])).toBe(true);

    const target = resolveWorkflowBoardProjectTarget({
      projectGroups: [group],
      projectKey: "repo:t3code",
      serverConfigs: configs,
      connectedEnvironmentIds: [ENV_LOCAL, ENV_REMOTE],
      primaryEnvironmentId: ENV_LOCAL,
    });
    expect(target.environmentId).toBe(ENV_REMOTE);
    expect(target.projectId).toBe(PROJECT_2);
    expect(target.availability).toBe("enabled");

    const explicitLocal = resolveWorkflowBoardProjectTarget({
      projectGroups: [group],
      projectKey: "repo:t3code",
      requestedEnvironmentId: ENV_LOCAL,
      serverConfigs: configs,
      connectedEnvironmentIds: [ENV_LOCAL],
      primaryEnvironmentId: ENV_LOCAL,
    });
    expect(explicitLocal.environmentId).toBe(ENV_LOCAL);
    expect(explicitLocal.availability).toBe("labs-disabled");

    const unsupportedConfigs = new Map<EnvironmentId, EnvironmentServerConfig>([
      [ENV_LOCAL, makeConfig({ workflowsEnabled: true, projectWorkflowBoard: undefined })],
    ]);
    const unsupported = resolveWorkflowBoardProjectTarget({
      projectGroups: [group],
      projectKey: "repo:t3code",
      requestedEnvironmentId: ENV_LOCAL,
      serverConfigs: unsupportedConfigs,
      connectedEnvironmentIds: [ENV_LOCAL],
      primaryEnvironmentId: ENV_LOCAL,
    });
    expect(unsupported.availability).toBe("unsupported");

    const loading = resolveWorkflowBoardProjectTarget({
      projectGroups: [group],
      projectKey: "repo:t3code",
      requestedEnvironmentId: ENV_LOCAL,
      serverConfigs: new Map(),
      connectedEnvironmentIds: [ENV_LOCAL],
      primaryEnvironmentId: ENV_LOCAL,
    });
    expect(loading.availability).toBe("loading");
  });

  it("builds workspace strategies without null branches and resolves default launch config", () => {
    expect(buildWorkspaceStrategy({ mode: "root", branch: "   " })).toEqual({ type: "root" });
    expect(buildWorkspaceStrategy({ mode: "root", branch: "main" })).toEqual({
      type: "root",
      branch: "main",
    });
    expect(
      buildWorkspaceStrategy({
        mode: "existing_worktree",
        worktreePath: " /repo/wt ",
        branch: "",
      }),
    ).toEqual({
      type: "existing_worktree",
      worktreePath: "/repo/wt",
    });
    expect(
      buildWorkspaceStrategy({
        mode: "worktree",
        baseRef: "",
        branch: "feat/board",
      }),
    ).toEqual({
      type: "worktree",
      baseRef: "",
      branch: "feat/board",
    });
    expect(
      normalizeWorkspaceStrategy({
        type: "worktree",
        baseRef: "  main  ",
        branch: "  feat/board ",
      }),
    ).toEqual({
      type: "worktree",
      baseRef: "main",
      branch: "feat/board",
    });

    expect(isWorkflowBoardWorkspaceStrategyValid({ type: "root" })).toBe(true);
    expect(isWorkflowBoardWorkspaceStrategyValid({ type: "worktree", baseRef: "HEAD" })).toBe(true);
    expect(isWorkflowBoardWorkspaceStrategyValid({ type: "worktree", baseRef: "   " })).toBe(false);
    expect(
      isWorkflowBoardWorkspaceStrategyValid(
        buildWorkspaceStrategy({ mode: "existing_worktree", worktreePath: "" }),
      ),
    ).toBe(false);
    expect(
      isWorkflowBoardWorkspaceStrategyValid(
        buildWorkspaceStrategy({ mode: "existing_worktree", worktreePath: "   " }),
      ),
    ).toBe(false);
    expect(
      isWorkflowBoardWorkspaceStrategyValid(
        buildWorkspaceStrategy({
          mode: "existing_worktree",
          worktreePath: "/repo/.worktrees/wt-1",
        }),
      ),
    ).toBe(true);

    const devReplayProvider = {
      instanceId: ProviderInstanceId.make("devReplay"),
      driver: "devReplay",
      enabled: true,
      installed: true,
      status: "ready",
      availability: "available",
      auth: { status: "authenticated" },
      models: [{ slug: "dev-replay", name: "Replay (dev)", isDefault: true, isCustom: false }],
    } as unknown as ServerProvider;

    const defaults = resolveDefaultBoardLaunchConfig({
      providers: [devReplayProvider],
      settings: DEFAULT_SERVER_SETTINGS,
      projectId: PROJECT_1,
    });
    expect(defaults.modelSelection).toEqual({
      instanceId: ProviderInstanceId.make("devReplay"),
      model: "dev-replay",
    });
    expect(defaults.interactionMode).toBe("default");
  });

  it("checks spec approval and plan launch preconditions and parses spec lists", () => {
    const baseDetail: Pick<WorkflowCardDetail, "spec" | "specApproval"> = {
      spec: {
        title: "Card",
        brief: "Brief",
        sourceType: "idea",
        assistanceMode: "balanced",
        acceptanceCriteria: [{ id: "AC-1", text: "The card can be approved" }],
        constraints: [],
        sourceRefs: [],
        wireframesIndex: null,
        specDigest: "sha256:abc",
        updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      },
      specApproval: null,
    };

    expect(canApproveWorkflowBoardSpec({ detail: baseDetail, canOperate: true })).toBe(true);
    expect(canApproveWorkflowBoardSpec({ detail: baseDetail, canOperate: false })).toBe(false);
    expect(
      canStartWorkflowBoardPlan({
        detail: baseDetail,
        canOperate: true,
        workflowsEnabled: true,
      }),
    ).toBe(false);

    const approvedDetail: Pick<WorkflowCardDetail, "spec" | "specApproval"> = {
      ...baseDetail,
      specApproval: {
        id: WorkflowBoardApprovalId.make("a-1"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(2),
        artifactDigest: "sha256:abc",
        specDigest: "sha256:abc",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
        approvedBy: "client",
      },
    };
    expect(canApproveWorkflowBoardSpec({ detail: approvedDetail, canOperate: true })).toBe(false);
    expect(
      canStartWorkflowBoardPlan({
        detail: approvedDetail,
        canOperate: true,
        workflowsEnabled: true,
      }),
    ).toBe(true);
    expect(
      canStartWorkflowBoardPlan({
        detail: approvedDetail,
        canOperate: true,
        workflowsEnabled: false,
      }),
    ).toBe(false);

    expect(parseSpecListLines(" a \n\n b ")).toEqual(["a", "b"]);
    const criteria = parseAcceptanceCriteriaLines("AC-10: First\nSecond item");
    expect(criteria).toEqual([
      { id: "AC-10", text: "First" },
      { id: "AC-2", text: "Second item" },
    ]);
    expect(formatAcceptanceCriteriaLines(criteria)).toBe("AC-10: First\nAC-2: Second item");
  });

  it("validates M2 intake paths, exploration/challenge readiness, Plan gates, and cross-column transitions", () => {
    expect(isSafeWorkflowBoardInputPath("docs/brief.md")).toBe(true);
    expect(isSafeWorkflowBoardInputPath("../secret.md")).toBe(false);
    expect(isSafeWorkflowBoardInputPath(".git/config")).toBe(false);
    expect(isSafeWorkflowBoardInputPath(".env.local")).toBe(false);
    expect(isSafeWorkflowBoardInputPath(".ssh/id_ed25519")).toBe(false);

    expect(parseArgvTokens('vp test run "src/my file.test.ts"')).toEqual([
      "vp",
      "test",
      "run",
      "src/my file.test.ts",
    ]);
    expect(formatArgvTokens(["vp", "test", "run", "src/my file.test.ts"])).toBe(
      'vp test run "src/my file.test.ts"',
    );
    expect(isSafeWorkflowBoardArgv(["vp", "test", "run"])).toBe(true);
    expect(isSafeWorkflowBoardArgv(["bash", "-c", "vp test run"])).toBe(false);
    expect(isSafeWorkflowBoardArgv(["vp", "test", "&&", "echo"])).toBe(false);

    expect(
      isManualChallengeFallbackValid({
        acknowledgementToken: "I acknowledge",
        answers: ["a1", "a2", "a3"],
        unresolvedConcernCount: 0,
      }),
    ).toBe(true);
    expect(
      isManualChallengeFallbackValid({
        acknowledgementToken: "ack",
        answers: ["a1", "a2", "a3"],
        unresolvedConcernCount: 0,
      }),
    ).toBe(false);
    expect(
      isManualChallengeFallbackValid({
        acknowledgementToken: "I acknowledge",
        answers: ["a1", " ", "a3"],
        unresolvedConcernCount: 0,
      }),
    ).toBe(false);
    expect(
      isManualChallengeFallbackValid({
        acknowledgementToken: "I acknowledge",
        answers: ["a1", "a2", "a3"],
        unresolvedConcernCount: 1,
      }),
    ).toBe(false);

    expect(
      canApproveWorkflowBoardSpec({
        detail: {
          spec: {
            title: "Spec",
            brief: "Brief",
            sourceType: "idea",
            assistanceMode: "balanced",
            acceptanceCriteria: [],
            constraints: [],
            sourceRefs: [],
            wireframesIndex: null,
            specDigest: "sha256:abc",
            updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
          },
          specApproval: null,
          exploration: {
            roundIndex: NonNegativeInt.make(1),
            pendingRoundPrompt: false,
            paused: false,
            unresolvedIds: ["DEC-1"],
            decisions: [],
          },
        },
        canOperate: true,
      }),
    ).toBe(false);

    expect(
      isPlanStrategyValid({
        strategy: "risk-first",
        strategyRationale: "",
        strategyMetadata: { uncertainty: "Unknown schema", probeTaskId: "TSK-01" },
      }),
    ).toBe(true);
    expect(
      isPlanStrategyValid({
        strategy: "risk-first",
        strategyRationale: "",
        strategyMetadata: { uncertainty: "Unknown schema" },
      }),
    ).toBe(false);
    expect(
      isPlanStrategyValid({
        strategy: "expand-contract",
        strategyRationale: "",
        strategyMetadata: {
          compatibilityWindow: "1 release",
          transitionStep: "Dual write",
          retirementPrereq: "0 readers",
        },
      }),
    ).toBe(true);
    expect(
      isPlanStrategyValid({
        strategy: "custom",
        strategyRationale: "Phased rollout",
        strategyMetadata: null,
      }),
    ).toBe(true);

    const completeDetail: WorkflowCardDetail = {
      cardId: WorkflowCardId.make("c-1"),
      projectId: PROJECT_1,
      title: "Feature",
      stage: "plan",
      status: "active",
      revision: PositiveInt.make(4),
      currentTask: null,
      blockerSummary: null,
      nextAction: "author_plan",
      specApprovedRevision: PositiveInt.make(3),
      specApprovalStatus: "valid",
      planApprovedRevision: null,
      planApprovalStatus: null,
      activeThreadId: null,
      createdAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      updatedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
      workspaceStrategy: { type: "root" },
      modelSelection: null,
      runtimeMode: "full-access",
      interactionMode: "default",
      spec: {
        title: "Feature",
        brief: "Brief",
        sourceType: "codebase",
        assistanceMode: "guided",
        acceptanceCriteria: [{ id: "AC-1", text: "Works" }],
        constraints: [],
        sourceRefs: [],
        wireframesIndex: null,
        specDigest: "sha256:spec",
        updatedAt: IsoDateTime.make("2026-11-24T00:00:00.000Z"),
      },
      specApproval: {
        id: WorkflowBoardApprovalId.make("apr-spec"),
        gate: "spec",
        status: "valid",
        revision: PositiveInt.make(3),
        artifactDigest: "sha256:spec",
        specDigest: "sha256:spec",
        wireframesDigest: null,
        approvedAt: IsoDateTime.make("2026-11-24T00:01:00.000Z"),
        approvedBy: "client",
      },
      plan: {
        lenses: [
          { lens: "ux", applicable: false, reason: "Backend only" },
          { lens: "functional", applicable: true, summary: "AC-1" },
          { lens: "architecture", applicable: true, summary: "Modular" },
          { lens: "quality", applicable: true, summary: "Unit tested" },
          { lens: "infrastructure", applicable: false, reason: "None" },
          { lens: "delivery", applicable: true, summary: "Single task" },
        ],
        alternatives: [
          { id: "a1", title: "Opt 1", summary: "Sum 1", tradeoffs: "Low risk" },
          { id: "a2", title: "Opt 2", summary: "Sum 2", tradeoffs: "High risk" },
        ],
        alternativesRevisionCount: NonNegativeInt.make(0),
        selectedAlternativeId: "a1",
        selectionRationale: "Lower risk",
        strategy: "outcomes",
        strategyRationale: null,
        strategyMetadata: null,
        granularity: "balanced",
        tasks: [
          {
            taskId: "TSK-01",
            title: "Build",
            filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/a.ts")],
            testingMethod: "test-first",
          },
        ],
        checks: [{ id: "CHK-01", argv: ["vp", "test", "run"] }],
        acMappings: [{ criterionId: "AC-1", method: "automated", checkIds: ["CHK-01"] }],
        planDigest: "sha256:plan",
        tasksDigest: "sha256:tasks",
        updatedAt: IsoDateTime.make("2026-11-24T00:02:00.000Z"),
      },
      stageRuns: [],
      revisions: [],
      artifactKeys: [
        WorkflowBoardArtifactKey.make("spec.md"),
        WorkflowBoardArtifactKey.make("plan.md"),
      ],
    };

    expect(collectPlanApprovalBlockers(completeDetail)).toEqual([]);
    expect(canApproveWorkflowBoardPlan({ detail: completeDetail, canOperate: true })).toBe(true);
    expect(isPlanOnlyEditPreservingSpecApproval(completeDetail)).toBe(true);
    expect(collectDownstreamInvalidationStates(completeDetail)).toEqual({
      specStale: false,
      planStale: false,
      challengeStale: false,
      externalEditNotice: false,
    });

    const missingAcDetail: WorkflowCardDetail = {
      ...completeDetail,
      plan: {
        ...completeDetail.plan!,
        acMappings: [],
      },
    };
    expect(collectPlanApprovalBlockers(missingAcDetail)).toContain("ac_mappings_incomplete");
    expect(canApproveWorkflowBoardPlan({ detail: missingAcDetail, canOperate: true })).toBe(false);

    expect(resolveCrossColumnTransition({ stage: "spec", status: "active" }, "plan")).toEqual({
      kind: "approve_spec",
      targetStage: "plan",
    });
    expect(resolveCrossColumnTransition({ stage: "plan", status: "active" }, "execute")).toEqual({
      kind: "approve_plan",
      targetStage: "execute",
    });
    expect(resolveCrossColumnTransition({ stage: "plan", status: "active" }, "spec")).toEqual({
      kind: "return_for_revision",
      targetStage: "spec",
    });
    expect(resolveCrossColumnTransition({ stage: "spec", status: "active" }, "execute")).toBeNull();
    expect(
      resolveCrossColumnTransition({ stage: "plan", status: "completed" }, "execute"),
    ).toBeNull();
    expect(validReturnTargetStages("execute")).toEqual(["spec", "plan"]);

    // Single-field external AC mappings strip empty optional strings cleanly
    const normalizedMappings = normalizeAcEvidenceMappings([
      { criterionId: "AC-1", method: "external", externalRef: "JIRA-42", externalSource: "   " },
      {
        criterionId: "AC-2",
        method: "external",
        externalRef: "",
        externalSource: "GitHub Actions",
      },
    ]);
    expect(normalizedMappings).toEqual([
      { criterionId: "AC-1", method: "external", externalRef: "JIRA-42" },
      { criterionId: "AC-2", method: "external", externalSource: "GitHub Actions" },
    ]);
    expect(
      collectPlanApprovalBlockers({
        ...completeDetail,
        plan: {
          ...completeDetail.plan!,
          checks: [{ id: "   ", argv: ["vp", "test", "run"] }],
        },
      }),
    ).toContain("checks_invalid");
  });

  it("handles M3 Execute/Ship transitions, DAG readiness, staleness, and fix pointers", () => {
    expect(resolveCrossColumnTransition({ stage: "execute", status: "active" }, "ship")).toEqual({
      kind: "approve_delivery",
      targetStage: "ship",
    });
    expect(
      resolveCrossColumnTransition({ stage: "execute", status: "completed" }, "ship"),
    ).toBeNull();

    const tasks: ReadonlyArray<WorkflowBoardTaskContract> = [
      {
        taskId: "TSK-01",
        title: "One",
        filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/one.ts")],
        blocks: ["TSK-02"],
        testingMethod: "test-first",
        status: "completed",
      },
      {
        taskId: "TSK-02",
        title: "Two",
        dependsOn: ["TSK-01"],
        filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/two.ts")],
        testingMethod: "test-first",
        status: "todo",
      },
      {
        taskId: "TSK-03",
        title: "Three",
        blockedBy: ["TSK-02"],
        filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/three.ts")],
        testingMethod: "test-first",
        status: "todo",
      },
    ];
    expect(taskDependencyIds(tasks[1]!, tasks)).toEqual(["TSK-01"]);
    expect(isTaskDagReady(tasks, [], "TSK-02")).toBe(true);
    expect(isTaskDagReady(tasks, [], "TSK-03")).toBe(false);
    expect(findInProgressTask(tasks, [])).toBeNull();

    const execInProgress = [
      {
        taskId: "TSK-02",
        status: "in_progress" as const,
        threadId: null,
        runId: null,
        stageRunId: null,
        worktreePath: null,
        branch: null,
        attempt: PositiveInt.make(1),
        startedAt: null,
        endedAt: null,
        lastError: null,
      },
    ];
    expect(findInProgressTask(tasks, execInProgress)).toBe("TSK-02");

    const readyCard = {
      stage: "execute" as const,
      status: "active" as const,
      planApproval: null,
      planApprovalStatus: "valid" as const,
      fileContractReports: [],
      incompleteJournalCount: 0,
      hasUnreconciledEdits: false,
      recoveryState: null,
    };
    expect(
      canStartExecuteTask({
        card: readyCard,
        taskId: "TSK-02",
        tasks,
        executions: [],
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
      }),
    ).toEqual({ allowed: true, reason: null });
    expect(
      canStartExecuteTask({
        card: readyCard,
        taskId: "TSK-03",
        tasks,
        executions: execInProgress,
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
      }).allowed,
    ).toBe(false);
    expect(
      canStartIndependentReview({
        card: readyCard,
        tasks,
        executions: execInProgress,
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
      }).allowed,
    ).toBe(false);
    expect(
      canStartIndependentReview({
        card: readyCard,
        tasks,
        executions: [],
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
      }),
    ).toEqual({ allowed: true, reason: null });

    expect(
      needsCardReconciliation({
        incompleteJournalCount: 1,
        hasUnreconciledEdits: false,
        recoveryState: null,
      }),
    ).toBe(true);
    expect(
      isCheckReceiptStale({ status: "passed", productSnapshotDigest: "sha256:old" }, "sha256:new"),
    ).toBe(true);
    expect(
      isCheckReceiptStale({ status: "passed", productSnapshotDigest: "sha256:new" }, "sha256:new"),
    ).toBe(false);
    expect(
      isReviewReportStale(
        { status: "completed", verdict: "pass", productSnapshotDigest: "sha256:old" },
        "sha256:new",
      ),
    ).toBe(true);
    expect(
      isAcObservationStale({ status: "pass", productSnapshotDigest: "sha256:old" }, "sha256:new"),
    ).toBe(true);

    expect(resolveFixPointerNavigation({ stage: "spec", tab: "spec", targetId: null })).toEqual({
      subTab: "spec",
      targetId: null,
    });
    expect(resolveFixPointerNavigation({ stage: "plan", tab: "plan", targetId: null })).toEqual({
      subTab: "plan",
      targetId: null,
    });
    expect(
      resolveFixPointerNavigation({ stage: "execute", tab: "recovery", targetId: "act-1" }),
    ).toEqual({ subTab: "execute", executeSection: "recovery", targetId: "act-1" });
    expect(
      resolveFixPointerNavigation({ stage: "execute", tab: "tasks", targetId: "TSK-02" }),
    ).toEqual({ subTab: "execute", executeSection: "tasks", targetId: "TSK-02" });
    expect(
      resolveFixPointerNavigation({ stage: "execute", tab: "checks", targetId: "CHK-01" }),
    ).toEqual({ subTab: "execute", executeSection: "checks", targetId: "CHK-01" });
    expect(
      resolveFixPointerNavigation({ stage: "execute", tab: "review", targetId: "fnd-1" }),
    ).toEqual({ subTab: "execute", executeSection: "review", targetId: "fnd-1" });
    expect(
      resolveFixPointerNavigation({ stage: "ship", tab: "acceptance", targetId: "AC-02" }),
    ).toEqual({ subTab: "ship", shipSection: "acceptance", targetId: "AC-02" });

    expect(
      buildAmendedTaskContractsWithUnexpectedFiles({
        tasks,
        sourceTaskId: "TSK-01",
        unexpectedFiles: [".git/config"],
      }),
    ).toEqual({ ok: false, reason: "unsafe_non_git_path" });

    expect(
      buildAmendedTaskContractsWithUnexpectedFiles({
        tasks,
        sourceTaskId: "TSK-01",
        unexpectedFiles: ["src/two.ts"],
      }),
    ).toEqual({ ok: false, reason: "contract_conflict" });

    const amendedSuccess = buildAmendedTaskContractsWithUnexpectedFiles({
      tasks,
      sourceTaskId: "TSK-01",
      unexpectedFiles: ["src/extra.ts"],
    });
    expect(amendedSuccess).toEqual({
      ok: true,
      tasks: [
        {
          ...tasks[0]!,
          filesAllowed: [
            WorkflowBoardNonGitArtifactKey.make("src/one.ts"),
            WorkflowBoardNonGitArtifactKey.make("src/extra.ts"),
          ],
        },
        tasks[1]!,
        tasks[2]!,
      ],
    });
  });

  it("handles M4 CLI-imported historical Execute gating, policy acknowledgement, blocked tasks, and policies fix pointer", () => {
    const cliTasks: ReadonlyArray<WorkflowBoardTaskContract> = [
      {
        taskId: "TSK-0001",
        title: "Task 1",
        filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/one.ts")],
        testingMethod: "test-first",
        status: "todo",
      },
      {
        taskId: "TSK-0002",
        title: "Blocked qualification task",
        filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/two.ts")],
        testingMethod: "test-first",
        status: "blocked",
      },
    ];
    const cliCard = {
      stage: "execute" as const,
      status: "blocked" as const,
      planApproval: null,
      planApprovalStatus: "historical" as const,
      fileContractReports: [],
      incompleteJournalCount: 0,
      hasUnreconciledEdits: false,
      recoveryState: null,
      cliImport: {
        sourceSystem: "legion-cli" as const,
        sourceCliDir: WorkflowBoardArtifactKey.make(".legion-cli"),
        sourceSpecId: "spec-t3-code",
        owner: "legioncode" as const,
        importedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        lastSyncedAt: IsoDateTime.make("2026-11-27T00:00:00.000Z"),
        sourceDigest: "sha256:cli",
        decisionIds: ["D-001"],
        assumptionIds: ["ASM-0001"],
        acceptanceCriterionIds: ["AC-P0-01"],
        historicalReceipts: [],
        unsupportedPolicies: [
          {
            key: "sandbox.requireHardened",
            category: "sandbox",
            summary: "Requires hardened sandbox",
            configuredValue: "true",
            status: "unsupported" as const,
            blocksExecution: true,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
          {
            key: "dashboard",
            category: "ui",
            summary: "Dashboard config",
            configuredValue: null,
            status: "unsupported" as const,
            blocksExecution: false,
            sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
          },
        ],
        acknowledgedPolicies: [],
        missingLogNotes: [],
        preservedArtifactPaths: [],
        assurancePolicySummary: null,
      },
    };

    expect(unacknowledgedBlockingCliPolicies(cliCard).map((p) => p.key)).toEqual([
      "sandbox.requireHardened",
    ]);

    // Unacknowledged blocking policies prevent Execute
    expect(
      canStartExecuteTask({
        card: cliCard,
        taskId: "TSK-0001",
        tasks: cliTasks,
        executions: [],
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
        unsupportedPoliciesAcknowledged: false,
      }),
    ).toEqual({
      allowed: false,
      reason: "Acknowledge unsupported CLI policies before starting Execute.",
    });

    // Historical planApprovalStatus is allowed for CLI-imported cards once policies are acknowledged
    expect(
      canStartExecuteTask({
        card: cliCard,
        taskId: "TSK-0001",
        tasks: cliTasks,
        executions: [],
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
        unsupportedPoliciesAcknowledged: true,
      }),
    ).toEqual({ allowed: true, reason: null });

    // Non-CLI card with historical planApprovalStatus remains blocked
    expect(
      canStartExecuteTask({
        card: { ...cliCard, cliImport: undefined },
        taskId: "TSK-0001",
        tasks: cliTasks,
        executions: [],
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
        unsupportedPoliciesAcknowledged: true,
      }).allowed,
    ).toBe(false);

    // Blocked task requires Retry task before Start task
    expect(
      canStartExecuteTask({
        card: cliCard,
        taskId: "TSK-0002",
        tasks: cliTasks,
        executions: [],
        canStartStage: true,
        workflowsEnabled: true,
        workspaceStrategyValid: true,
        hasModelSelection: true,
        unsupportedPoliciesAcknowledged: true,
      }),
    ).toEqual({
      allowed: false,
      reason: "Retry this blocked task before starting execution.",
    });

    // Fix pointer for policy_unacknowledged navigates to execute policies section
    expect(
      resolveFixPointerNavigation({
        stage: "execute",
        tab: "policies",
        targetId: "sandbox.requireHardened",
      }),
    ).toEqual({
      subTab: "execute",
      executeSection: "policies",
      targetId: "sandbox.requireHardened",
    });
  });
});

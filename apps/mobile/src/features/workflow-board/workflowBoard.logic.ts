import type { EnvironmentProject } from "@legioncode/client-runtime/state/shell";
import {
  WORKFLOW_BOARD_PLAN_LENSES,
  WORKFLOW_BOARD_STAGES,
  canApproveWorkflowBoardDelivery,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  collectDownstreamInvalidationStates,
  collectPlanApprovalBlockers,
  groupWorkflowBoardCards,
  isAcObservationStale,
  isCheckReceiptStale,
  isProjectWorkflowBoardSupported,
  isReviewReportStale,
  isWorkflowBoardCardShipped,
  projectWorkflowBoardAvailability,
  resolveFixPointerNavigation,
  validReturnTargetStages,
  workflowBoardAcEvidenceStatusLabel,
  workflowBoardAvailabilityNotice,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardCheckStatusLabel,
  workflowBoardCliImportHistoricalReceiptKindLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardHasHistoricalEvidence,
  workflowBoardLatestStageRun,
  workflowBoardNeedsRecovery,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardProjectNotFoundNotice,
  workflowBoardReadPermissionNotice,
  workflowBoardRecoverySummary,
  workflowBoardReviewVerdictLabel,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
  workflowBoardStaleEvidenceSummary,
  workflowBoardTaskStatusMap,
  workflowBoardUnacknowledgedBlockingPolicies,
  workflowBoardUnresolvedFindings,
  type PlanApprovalBlockerCode,
  type WorkflowBoardAvailability,
  type WorkflowBoardColumns,
  type WorkflowBoardFixNavigation,
} from "@legioncode/client-runtime/workflow-board";
import type {
  EnvironmentId,
  ProjectId,
  ThreadId,
  WorkflowBoardCardSummary,
  WorkflowBoardFindingResolveDisposition,
  WorkflowBoardManualAcceptanceVerdict,
  WorkflowBoardReturnTargetStage,
  WorkflowBoardStage,
  WorkflowBoardStageRunStage,
  WorkflowCardDetail,
} from "@legioncode/contracts";
import { msg } from "gt-react-native";

import { m, type MessageResolver } from "../../i18n/messages";
import type { EnvironmentPresentation } from "../../state/environments";

export {
  WORKFLOW_BOARD_PLAN_LENSES,
  WORKFLOW_BOARD_STAGES,
  canApproveWorkflowBoardDelivery,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  collectDownstreamInvalidationStates,
  collectPlanApprovalBlockers,
  groupWorkflowBoardCards,
  isAcObservationStale,
  isCheckReceiptStale,
  isProjectWorkflowBoardSupported,
  isReviewReportStale,
  isWorkflowBoardCardShipped,
  projectWorkflowBoardAvailability,
  resolveFixPointerNavigation,
  validReturnTargetStages,
  workflowBoardAcEvidenceStatusLabel,
  workflowBoardAvailabilityNotice,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardCheckStatusLabel,
  workflowBoardCliImportHistoricalReceiptKindLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardHasHistoricalEvidence,
  workflowBoardLatestStageRun,
  workflowBoardNeedsRecovery,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardProjectNotFoundNotice,
  workflowBoardReadPermissionNotice,
  workflowBoardRecoverySummary,
  workflowBoardReviewVerdictLabel,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
  workflowBoardStaleEvidenceSummary,
  workflowBoardTaskStatusMap,
  workflowBoardUnacknowledgedBlockingPolicies,
  workflowBoardUnresolvedFindings,
  type PlanApprovalBlockerCode,
  type WorkflowBoardAvailability,
  type WorkflowBoardColumns,
  type WorkflowBoardFixNavigation,
};

export type MobileBoardStageFilter = WorkflowBoardStage | "completed";
export type MobileCardSubTab = "spec" | "plan" | "execute" | "ship" | "history";

export const MOBILE_BOARD_STAGE_FILTERS: readonly MobileBoardStageFilter[] = [
  "spec",
  "plan",
  "execute",
  "ship",
  "completed",
];

export const MOBILE_CARD_SUB_TABS: readonly MobileCardSubTab[] = [
  "spec",
  "plan",
  "execute",
  "ship",
  "history",
];

const AVAILABILITY_RANK: Record<WorkflowBoardAvailability, number> = {
  enabled: 0,
  "labs-disabled": 1,
  loading: 2,
  disconnected: 3,
  unsupported: 4,
};

export interface MobileWorkflowBoardCandidate {
  readonly key: string;
  readonly environmentId: EnvironmentId;
  readonly environmentLabel: string;
  readonly projectId: ProjectId;
  readonly projectTitle: string;
  readonly workspaceRoot: string;
  readonly availability: WorkflowBoardAvailability;
}

export interface MobileWorkflowBoardTarget {
  readonly loading: boolean;
  readonly candidates: ReadonlyArray<MobileWorkflowBoardCandidate>;
  readonly selected: MobileWorkflowBoardCandidate | null;
  readonly availability: WorkflowBoardAvailability;
  readonly missingRequestedProject: boolean;
}

export type MobileEnvironmentInput = Pick<
  EnvironmentPresentation,
  "environmentId" | "label" | "connection" | "serverConfig"
>;

export type MobileProjectInput = Pick<
  EnvironmentProject,
  "environmentId" | "id" | "title" | "workspaceRoot" | "archivedAt"
>;

export function environmentWorkflowBoardAvailability(
  environment: MobileEnvironmentInput,
  isReady: boolean,
): WorkflowBoardAvailability {
  if (!isReady || environment.connection.phase === "connecting") {
    return "loading";
  }
  return projectWorkflowBoardAvailability({
    connected: environment.connection.phase === "connected",
    configLoaded: environment.serverConfig !== null,
    settings: environment.serverConfig?.settings,
    capabilities: environment.serverConfig?.environment.capabilities,
  });
}

export function resolvePreferredProjectCandidate(
  candidates: ReadonlyArray<MobileWorkflowBoardCandidate>,
): MobileWorkflowBoardCandidate | null {
  if (candidates.length === 0) return null;
  return (
    [...candidates].sort((left, right) => {
      const rankDiff = AVAILABILITY_RANK[left.availability] - AVAILABILITY_RANK[right.availability];
      if (rankDiff !== 0) return rankDiff;
      const titleDiff = left.projectTitle.localeCompare(right.projectTitle);
      if (titleDiff !== 0) return titleDiff;
      const envDiff = left.environmentLabel.localeCompare(right.environmentLabel);
      if (envDiff !== 0) return envDiff;
      return left.key.localeCompare(right.key);
    })[0] ?? null
  );
}

export function resolveMobileWorkflowBoardTarget(input: {
  readonly isReady: boolean;
  readonly environments: ReadonlyArray<MobileEnvironmentInput>;
  readonly projects: ReadonlyArray<MobileProjectInput>;
  readonly requestedEnvironmentId?: string | null | undefined;
  readonly requestedProjectId?: string | null | undefined;
}): MobileWorkflowBoardTarget {
  const envById = new Map(input.environments.map((env) => [env.environmentId, env]));
  const reqEnv = input.requestedEnvironmentId?.trim() || null;
  const reqProj = input.requestedProjectId?.trim() || null;

  const toCandidate = (project: MobileProjectInput): MobileWorkflowBoardCandidate | null => {
    const env = envById.get(project.environmentId);
    if (env === undefined) return null;
    return {
      key: `${project.environmentId}:${project.id}`,
      environmentId: project.environmentId,
      environmentLabel: env.label,
      projectId: project.id,
      projectTitle: project.title,
      workspaceRoot: project.workspaceRoot,
      availability: environmentWorkflowBoardAvailability(env, input.isReady),
    };
  };

  const unarchivedCandidates = input.projects
    .filter((project) => project.archivedAt === null)
    .map(toCandidate)
    .filter((candidate): candidate is MobileWorkflowBoardCandidate => candidate !== null);

  const candidates = [...unarchivedCandidates].sort((left, right) => {
    const rankDiff = AVAILABILITY_RANK[left.availability] - AVAILABILITY_RANK[right.availability];
    if (rankDiff !== 0) return rankDiff;
    const titleDiff = left.projectTitle.localeCompare(right.projectTitle);
    if (titleDiff !== 0) return titleDiff;
    const envDiff = left.environmentLabel.localeCompare(right.environmentLabel);
    if (envDiff !== 0) return envDiff;
    return left.key.localeCompare(right.key);
  });

  if (reqEnv !== null || reqProj !== null) {
    const allCandidates = input.projects
      .map(toCandidate)
      .filter((candidate): candidate is MobileWorkflowBoardCandidate => candidate !== null);
    const matching = allCandidates.filter(
      (candidate) =>
        (reqEnv === null || candidate.environmentId === reqEnv) &&
        (reqProj === null || candidate.projectId === reqProj),
    );
    const selected = resolvePreferredProjectCandidate(matching);
    if (selected !== null) {
      return {
        loading: selected.availability === "loading",
        candidates,
        selected,
        availability: selected.availability,
        missingRequestedProject: false,
      };
    }

    const requestedEnv = reqEnv !== null ? envById.get(reqEnv as EnvironmentId) : undefined;
    const anyConnecting = input.environments.some((env) => env.connection.phase === "connecting");
    if (
      !input.isReady ||
      requestedEnv?.connection.phase === "connecting" ||
      (reqEnv === null && anyConnecting)
    ) {
      return {
        loading: true,
        candidates,
        selected: null,
        availability: "loading",
        missingRequestedProject: false,
      };
    }
    if (requestedEnv !== undefined) {
      const envAvailability = environmentWorkflowBoardAvailability(requestedEnv, input.isReady);
      if (envAvailability !== "enabled") {
        return {
          loading: envAvailability === "loading",
          candidates,
          selected: null,
          availability: envAvailability,
          missingRequestedProject: false,
        };
      }
    }
    return {
      loading: false,
      candidates,
      selected: null,
      availability: "disconnected",
      missingRequestedProject: true,
    };
  }

  const selected = candidates[0] ?? null;
  if (!input.isReady) {
    return {
      loading: true,
      candidates,
      selected,
      availability: "loading",
      missingRequestedProject: false,
    };
  }
  if (selected !== null) {
    return {
      loading: selected.availability === "loading",
      candidates,
      selected,
      availability: selected.availability,
      missingRequestedProject: false,
    };
  }

  const envAvailabilities = input.environments.map((env) =>
    environmentWorkflowBoardAvailability(env, input.isReady),
  );
  if (envAvailabilities.includes("loading")) {
    return {
      loading: true,
      candidates,
      selected: null,
      availability: "loading",
      missingRequestedProject: false,
    };
  }
  if (envAvailabilities.includes("enabled")) {
    return {
      loading: false,
      candidates,
      selected: null,
      availability: "enabled",
      missingRequestedProject: true,
    };
  }
  if (envAvailabilities.includes("labs-disabled")) {
    return {
      loading: false,
      candidates,
      selected: null,
      availability: "labs-disabled",
      missingRequestedProject: false,
    };
  }
  if (envAvailabilities.includes("unsupported")) {
    return {
      loading: false,
      candidates,
      selected: null,
      availability: "unsupported",
      missingRequestedProject: false,
    };
  }
  return {
    loading: false,
    candidates,
    selected: null,
    availability: "disconnected",
    missingRequestedProject: false,
  };
}

export function isMobileBoardStageFilter(value: unknown): value is MobileBoardStageFilter {
  return (
    value === "spec" ||
    value === "plan" ||
    value === "execute" ||
    value === "ship" ||
    value === "completed"
  );
}

export function selectMobileBoardStageCards(
  columns: WorkflowBoardColumns,
  filter: MobileBoardStageFilter,
): ReadonlyArray<WorkflowBoardCardSummary> {
  return columns[filter];
}

export function defaultMobileBoardStageFilter(
  columns: WorkflowBoardColumns,
  requestedStage?: string | undefined,
): MobileBoardStageFilter {
  if (isMobileBoardStageFilter(requestedStage)) {
    return requestedStage;
  }
  for (const stage of MOBILE_BOARD_STAGE_FILTERS) {
    if (columns[stage].length > 0) return stage;
  }
  return "spec";
}

export function mobileBoardStageFilterLabel(
  filter: MobileBoardStageFilter,
  resolve: MessageResolver = m,
): string {
  if (filter === "completed") {
    return resolve(msg("Completed", { $context: "workflow board completed stage tab" }));
  }
  return workflowBoardStageLabel(filter, resolve);
}

export function mobileCardSubTabLabel(tab: MobileCardSubTab, resolve: MessageResolver = m): string {
  if (tab === "history") {
    return resolve(msg("History", { $context: "workflow card detail sub-tab" }));
  }
  return workflowBoardStageLabel(tab, resolve);
}

export function defaultMobileCardSubTab(
  card: Pick<WorkflowCardDetail, "stage" | "status">,
  requestedStage?: string | undefined,
): MobileCardSubTab {
  if (
    requestedStage === "spec" ||
    requestedStage === "plan" ||
    requestedStage === "execute" ||
    requestedStage === "ship" ||
    requestedStage === "history"
  ) {
    return requestedStage;
  }
  if (card.status === "completed") return "ship";
  return card.stage;
}

export interface MobileCardThreadLink {
  readonly key: string;
  readonly threadId: ThreadId;
  readonly kind: "active" | "spec" | "plan" | "execute" | "review" | "ship" | "task";
  readonly taskId: string | null;
}

export function collectMobileCardThreadLinks(
  card: Pick<
    WorkflowCardDetail,
    "activeThreadId" | "stageRuns" | "taskExecutions" | "reviewReport"
  >,
): ReadonlyArray<MobileCardThreadLink> {
  const seen = new Set<string>();
  const links: MobileCardThreadLink[] = [];

  const pushLink = (link: MobileCardThreadLink) => {
    if (seen.has(link.threadId)) return;
    seen.add(link.threadId);
    links.push(link);
  };

  const isNonEmptyThreadId = (value: ThreadId | null | undefined): value is ThreadId =>
    typeof value === "string" && value.trim().length > 0;

  if (isNonEmptyThreadId(card.activeThreadId)) {
    pushLink({
      key: `active:${card.activeThreadId}`,
      threadId: card.activeThreadId,
      kind: "active",
      taskId: null,
    });
  }

  const stageOrder: readonly WorkflowBoardStageRunStage[] = [
    "spec",
    "plan",
    "execute",
    "review",
    "ship",
  ];
  for (const stage of stageOrder) {
    const run = workflowBoardLatestStageRun(card, stage);
    if (isNonEmptyThreadId(run?.threadId)) {
      pushLink({
        key: `${stage}:${run.threadId}`,
        threadId: run.threadId,
        kind: stage,
        taskId: run.taskId ?? null,
      });
    }
  }

  if (isNonEmptyThreadId(card.reviewReport?.threadId)) {
    pushLink({
      key: `review-report:${card.reviewReport.threadId}`,
      threadId: card.reviewReport.threadId,
      kind: "review",
      taskId: null,
    });
  }

  for (const exec of card.taskExecutions ?? []) {
    if (isNonEmptyThreadId(exec.threadId)) {
      pushLink({
        key: `task:${exec.taskId}:${exec.threadId}`,
        threadId: exec.threadId,
        kind: "task",
        taskId: exec.taskId,
      });
    }
  }

  return links;
}

export function mobileCardThreadLinkLabel(
  link: MobileCardThreadLink,
  resolve: MessageResolver = m,
): string {
  switch (link.kind) {
    case "active":
      return resolve(msg("Open active thread"));
    case "spec":
      return resolve(msg("Open Spec thread"));
    case "plan":
      return resolve(msg("Open Plan thread"));
    case "execute":
      return link.taskId
        ? resolve(msg("Open Execute thread ({taskId})", { taskId: link.taskId }))
        : resolve(msg("Open Execute thread"));
    case "review":
      return resolve(msg("Open Review thread"));
    case "ship":
      return resolve(msg("Open Ship thread"));
    case "task":
      return resolve(msg("Open task {taskId} thread", { taskId: link.taskId ?? "" }));
  }
}

export function mobilePlanApprovalBlockerMessage(
  code: PlanApprovalBlockerCode,
  resolve: MessageResolver = m,
): string {
  switch (code) {
    case "spec_not_approved":
      return resolve(msg("Approve the spec before approving the plan."));
    case "plan_not_saved":
      return resolve(msg("Save the plan before approving."));
    case "lenses_incomplete":
      return resolve(msg("Complete all six design lens applicability decisions."));
    case "alternatives_incomplete":
      return resolve(
        msg("Provide 2 distinct alternatives, pick one, and record the selection rationale."),
      );
    case "strategy_incomplete":
      return resolve(msg("Complete required planning strategy fields."));
    case "granularity_missing":
      return resolve(msg("Choose a task granularity."));
    case "tasks_incomplete":
      return resolve(msg("Add at least one valid task with exclusive file ownership."));
    case "checks_invalid":
      return resolve(msg("Fix invalid argv check commands."));
    case "ac_mappings_incomplete":
      return resolve(
        msg("Map 100% of acceptance criteria to automated, manual, or external evidence."),
      );
  }
}

export function mobileFindingDispositionLabel(
  disposition: WorkflowBoardFindingResolveDisposition,
  resolve: MessageResolver = m,
): string {
  switch (disposition) {
    case "fix":
      return resolve(msg("Fix in code", { $context: "workflow board review finding disposition" }));
    case "accept_risk":
      return resolve(msg("Accept risk", { $context: "workflow board review finding disposition" }));
    case "dismiss":
      return resolve(msg("Dismiss", { $context: "workflow board review finding disposition" }));
    case "defer":
      return resolve(msg("Defer", { $context: "workflow board review finding disposition" }));
  }
}

export function mobileManualAcceptanceVerdictLabel(
  verdict: WorkflowBoardManualAcceptanceVerdict,
  resolve: MessageResolver = m,
): string {
  switch (verdict) {
    case "pass":
      return resolve(msg("Pass", { $context: "workflow board manual acceptance verdict" }));
    case "fail":
      return resolve(msg("Fail", { $context: "workflow board manual acceptance verdict" }));
    case "not-applicable":
      return resolve(
        msg("Not applicable", { $context: "workflow board manual acceptance verdict" }),
      );
  }
}

export function mobileReturnTargetStageLabel(
  stage: WorkflowBoardReturnTargetStage,
  resolve: MessageResolver = m,
): string {
  return workflowBoardStageLabel(stage, resolve);
}

export function workflowCardStateKey(
  environmentId: string,
  projectId: string,
  cardId: string,
): string {
  return `${environmentId}:${projectId}:${cardId}`;
}

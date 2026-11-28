import type { EnvironmentServerConfig } from "@legioncode/client-runtime/connection";
import {
  WORKFLOW_BOARD_PLAN_LENSES,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  collectDownstreamInvalidationStates,
  collectPlanApprovalBlockers,
  isAcObservationStale,
  isCheckReceiptStale,
  isPlanStrategyValid,
  isProjectWorkflowBoardEnabled,
  isReviewReportStale,
  isSafeWorkflowBoardArgv,
  normalizeAcEvidenceMappings,
  projectWorkflowBoardAvailability,
  resolveFixPointerNavigation,
  validReturnTargetStages,
  workflowBoardNeedsRecovery,
  workflowBoardUnacknowledgedBlockingPolicies,
  type PlanApprovalBlockerCode,
  type WorkflowBoardAvailability,
  type WorkflowBoardFixNavigation,
} from "@legioncode/client-runtime/workflow-board";
import {
  type EnvironmentId,
  type ModelSelection,
  type OrchestrationV2ThreadLaunchWorkspaceStrategy,
  ProjectId,
  type ProviderInteractionMode,
  RuntimeMode,
  ServerProvider,
  ServerSettings,
  WorkflowBoardNonGitArtifactKey,
  type WorkflowBoardCardSummary,
  type WorkflowBoardReturnTargetStage,
  WorkflowBoardSafeInputPath,
  WorkflowBoardStage,
  type WorkflowBoardTaskContract,
  WorkflowBoardTaskContractsArray,
  type WorkflowBoardTaskExecutionState,
  type WorkflowBoardTaskStatus,
  WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Schema from "effect/Schema";
import { msg } from "gt-react";

import { m, type MessageResolver } from "../../i18n/messages";

import {
  getDefaultProviderInstanceModel,
  providerInstanceEntriesFor,
  resolveSelectableProviderInstanceEntry,
  type ProviderInstanceEntry,
} from "../../providerInstances";
import type {
  SidebarProjectGroupMember,
  SidebarProjectSnapshot,
} from "../../sidebarProjectGrouping";

export {
  WORKFLOW_BOARD_PLAN_LENSES,
  canApproveWorkflowBoardPlan,
  canApproveWorkflowBoardSpec,
  collectDownstreamInvalidationStates,
  collectPlanApprovalBlockers,
  isAcObservationStale,
  isCheckReceiptStale,
  isPlanStrategyValid,
  isReviewReportStale,
  isSafeWorkflowBoardArgv,
  normalizeAcEvidenceMappings,
  resolveFixPointerNavigation,
  validReturnTargetStages,
  type PlanApprovalBlockerCode,
  type WorkflowBoardFixNavigation,
};

export type WorkflowBoardTab = "overview" | WorkflowBoardStage;

export interface WorkflowBoardSearch {
  readonly env?: string;
  readonly projectId?: ProjectId;
  readonly card?: string;
  readonly stage?: WorkflowBoardStage;
}

const VALID_STAGES = new Set<WorkflowBoardStage>(["spec", "plan", "execute", "ship"]);

function isWorkflowBoardStage(value: unknown): value is WorkflowBoardStage {
  return typeof value === "string" && VALID_STAGES.has(value as WorkflowBoardStage);
}

export function parseWorkflowBoardSearch(raw: Record<string, unknown>): WorkflowBoardSearch {
  return {
    ...(typeof raw.env === "string" && raw.env.trim() !== ""
      ? { env: raw.env.trim().slice(0, 200) }
      : {}),
    ...(typeof raw.projectId === "string" && raw.projectId.trim() !== ""
      ? { projectId: ProjectId.make(raw.projectId.trim().slice(0, 200)) }
      : {}),
    ...(typeof raw.card === "string" && raw.card.trim() !== ""
      ? { card: raw.card.trim().slice(0, 200) }
      : {}),
    ...(isWorkflowBoardStage(raw.stage) ? { stage: raw.stage } : {}),
  };
}

export interface WorkflowBoardProjectTarget {
  readonly group: SidebarProjectSnapshot | null;
  readonly member: SidebarProjectGroupMember | null;
  readonly environmentId: EnvironmentId | null;
  readonly projectId: ProjectId | null;
  readonly availability: WorkflowBoardAvailability;
  readonly candidates: ReadonlyArray<SidebarProjectGroupMember>;
}

function memberAvailability(
  member: Pick<SidebarProjectGroupMember, "environmentId">,
  serverConfigs: ReadonlyMap<EnvironmentId, EnvironmentServerConfig>,
  connectedEnvironmentIds: ReadonlyArray<EnvironmentId>,
): WorkflowBoardAvailability {
  const config = serverConfigs.get(member.environmentId);
  return projectWorkflowBoardAvailability({
    connected: connectedEnvironmentIds.includes(member.environmentId),
    configLoaded: config !== undefined,
    settings: config?.settings,
    capabilities: config?.environment.capabilities,
  });
}

export function isProjectGroupWorkflowBoardEnabled(
  group: Pick<SidebarProjectSnapshot, "memberProjects">,
  serverConfigs: ReadonlyMap<EnvironmentId, EnvironmentServerConfig>,
  connectedEnvironmentIds: ReadonlyArray<EnvironmentId>,
): boolean {
  return group.memberProjects.some((member) => {
    if (!connectedEnvironmentIds.includes(member.environmentId)) return false;
    const config = serverConfigs.get(member.environmentId);
    return isProjectWorkflowBoardEnabled({
      settings: config?.settings,
      capabilities: config?.environment.capabilities,
    });
  });
}

function resolvePreferredWorkflowBoardMember(input: {
  readonly group: Pick<SidebarProjectSnapshot, "environmentId" | "id" | "memberProjects">;
  readonly serverConfigs: ReadonlyMap<EnvironmentId, EnvironmentServerConfig>;
  readonly connectedEnvironmentIds: ReadonlyArray<EnvironmentId>;
  readonly primaryEnvironmentId: EnvironmentId | null;
}): SidebarProjectGroupMember | null {
  const { group, serverConfigs, connectedEnvironmentIds, primaryEnvironmentId } = input;
  const enabled = group.memberProjects.filter(
    (member) => memberAvailability(member, serverConfigs, connectedEnvironmentIds) === "enabled",
  );
  if (enabled.length > 0) {
    return (
      enabled.find((member) => member.environmentId === primaryEnvironmentId) ??
      enabled.find(
        (member) => member.environmentId === group.environmentId && member.id === group.id,
      ) ??
      enabled[0]!
    );
  }
  const connected = group.memberProjects.filter((member) =>
    connectedEnvironmentIds.includes(member.environmentId),
  );
  if (connected.length > 0) {
    return (
      connected.find((member) => member.environmentId === primaryEnvironmentId) ??
      connected.find(
        (member) => member.environmentId === group.environmentId && member.id === group.id,
      ) ??
      connected[0]!
    );
  }
  return group.memberProjects[0] ?? null;
}

export function resolveWorkflowBoardProjectTarget(input: {
  readonly projectGroups: ReadonlyArray<SidebarProjectSnapshot>;
  readonly projectKey: string;
  readonly requestedEnvironmentId?: string | undefined;
  readonly requestedProjectId?: string | undefined;
  readonly serverConfigs: ReadonlyMap<EnvironmentId, EnvironmentServerConfig>;
  readonly connectedEnvironmentIds: ReadonlyArray<EnvironmentId>;
  readonly primaryEnvironmentId: EnvironmentId | null;
}): WorkflowBoardProjectTarget {
  const group =
    input.projectGroups.find((candidate) => candidate.projectKey === input.projectKey) ??
    input.projectGroups.find((candidate) =>
      candidate.memberProjects.some(
        (member) =>
          member.physicalProjectKey === input.projectKey || member.id === input.projectKey,
      ),
    ) ??
    null;

  if (group === null) {
    return {
      group: null,
      member: null,
      environmentId: null,
      projectId: null,
      availability: "disconnected",
      candidates: [],
    };
  }

  const explicit =
    input.requestedEnvironmentId !== undefined || input.requestedProjectId !== undefined
      ? (group.memberProjects.find(
          (candidate) =>
            (input.requestedEnvironmentId === undefined ||
              candidate.environmentId === input.requestedEnvironmentId) &&
            (input.requestedProjectId === undefined || candidate.id === input.requestedProjectId),
        ) ?? null)
      : null;

  const member =
    explicit ??
    resolvePreferredWorkflowBoardMember({
      group,
      serverConfigs: input.serverConfigs,
      connectedEnvironmentIds: input.connectedEnvironmentIds,
      primaryEnvironmentId: input.primaryEnvironmentId,
    });

  if (member === null) {
    return {
      group,
      member: null,
      environmentId: null,
      projectId: null,
      availability: "disconnected",
      candidates: group.memberProjects,
    };
  }

  return {
    group,
    member,
    environmentId: member.environmentId,
    projectId: member.id,
    availability: memberAvailability(member, input.serverConfigs, input.connectedEnvironmentIds),
    candidates: group.memberProjects,
  };
}

export type BoardWorkspaceStrategyMode = "root" | "worktree" | "existing_worktree";

export function buildWorkspaceStrategy(input: {
  readonly mode: BoardWorkspaceStrategyMode;
  readonly baseRef?: string | undefined;
  readonly worktreePath?: string | undefined;
  readonly branch?: string | undefined;
}): OrchestrationV2ThreadLaunchWorkspaceStrategy {
  const branch = input.branch?.trim();
  switch (input.mode) {
    case "root":
      return branch ? { type: "root", branch } : { type: "root" };
    case "existing_worktree":
      return {
        type: "existing_worktree",
        worktreePath: input.worktreePath?.trim() ?? "",
        ...(branch ? { branch } : {}),
      };
    case "worktree":
      return {
        type: "worktree",
        // Keep raw baseRef during editing so clearing "HEAD" does not snap back mid-typing.
        baseRef: input.baseRef ?? "HEAD",
        ...(branch ? { branch } : {}),
      };
  }
}

export function normalizeWorkspaceStrategy(
  strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
): OrchestrationV2ThreadLaunchWorkspaceStrategy {
  const branch = strategy.branch?.trim();
  switch (strategy.type) {
    case "root":
      return branch ? { type: "root", branch } : { type: "root" };
    case "existing_worktree":
      return {
        type: "existing_worktree",
        worktreePath: strategy.worktreePath.trim(),
        ...(branch ? { branch } : {}),
      };
    case "worktree":
      return {
        type: "worktree",
        baseRef: strategy.baseRef.trim(),
        ...(branch ? { branch } : {}),
      };
  }
}

export function isWorkflowBoardWorkspaceStrategyValid(
  strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy,
): boolean {
  switch (strategy.type) {
    case "root":
      return true;
    case "existing_worktree":
      return strategy.worktreePath.trim() !== "";
    case "worktree":
      return strategy.baseRef.trim() !== "";
  }
}

export function resolveBoardProviderEntries(
  providers: ReadonlyArray<ServerProvider>,
  settings: ServerSettings,
  projectId: ProjectId | null,
): ReadonlyArray<ProviderInstanceEntry> {
  return providerInstanceEntriesFor(providers, settings, projectId).map((entry) =>
    entry.driverKind === "devReplay" && entry.snapshot.enabled && !entry.enabled
      ? { ...entry, enabled: true }
      : entry,
  );
}

export function resolveDefaultBoardLaunchConfig(input: {
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly projectId: ProjectId | null;
  readonly card?: Pick<
    WorkflowCardDetail,
    "modelSelection" | "runtimeMode" | "interactionMode" | "workspaceStrategy"
  > | null;
}): {
  readonly modelSelection: ModelSelection | null;
  readonly runtimeMode: RuntimeMode;
  readonly interactionMode: ProviderInteractionMode;
  readonly workspaceStrategy: OrchestrationV2ThreadLaunchWorkspaceStrategy;
} {
  const card = input.card ?? null;
  const entries = resolveBoardProviderEntries(input.providers, input.settings, input.projectId);
  const selectedEntry =
    resolveSelectableProviderInstanceEntry(
      entries.filter((candidate) => candidate.driverKind === entries[0]?.driverKind),
      undefined,
    ) ?? resolveSelectableProviderInstanceEntry(entries, undefined);
  const defaultModel = selectedEntry
    ? (getDefaultProviderInstanceModel(input.providers, selectedEntry.instanceId) ??
      selectedEntry.models[0]?.slug)
    : undefined;
  const fallbackSelection =
    selectedEntry && defaultModel
      ? { instanceId: selectedEntry.instanceId, model: defaultModel }
      : null;
  return {
    modelSelection: card?.modelSelection ?? fallbackSelection,
    runtimeMode: card?.runtimeMode ?? input.settings.defaultRuntimeMode ?? "full-access",
    interactionMode: card?.interactionMode ?? "default",
    workspaceStrategy: card?.workspaceStrategy ?? { type: "root" },
  };
}

export function canStartWorkflowBoardPlan(input: {
  readonly detail: Pick<WorkflowCardDetail, "specApproval"> | null;
  readonly canOperate: boolean;
  readonly workflowsEnabled: boolean;
}): boolean {
  return (
    input.canOperate &&
    input.workflowsEnabled &&
    input.detail !== null &&
    input.detail.specApproval?.status === "valid"
  );
}

const isSafeInputPathSchema = Schema.is(WorkflowBoardSafeInputPath);
const isNonGitArtifactKeySchema = Schema.is(WorkflowBoardNonGitArtifactKey);
const isValidTaskContractsArray = Schema.is(WorkflowBoardTaskContractsArray);

export function isSafeWorkflowBoardInputPath(path: string): boolean {
  return isSafeInputPathSchema(path.trim());
}

export function isSafeWorkflowBoardNonGitPath(path: string): boolean {
  return isNonGitArtifactKeySchema(path.trim());
}

export type BuildAmendedTasksResult =
  | { readonly ok: true; readonly tasks: ReadonlyArray<WorkflowBoardTaskContract> }
  | { readonly ok: false; readonly reason: "unsafe_non_git_path" | "contract_conflict" };

export function buildAmendedTaskContractsWithUnexpectedFiles(input: {
  readonly tasks: ReadonlyArray<WorkflowBoardTaskContract>;
  readonly sourceTaskId: string;
  readonly unexpectedFiles: ReadonlyArray<string>;
}): BuildAmendedTasksResult {
  const normalizedUnexpected = input.unexpectedFiles.map((filePath) => filePath.trim());
  if (normalizedUnexpected.some((filePath) => !isSafeWorkflowBoardNonGitPath(filePath))) {
    return { ok: false, reason: "unsafe_non_git_path" };
  }
  const safeUnexpected = normalizedUnexpected.map((filePath) =>
    WorkflowBoardNonGitArtifactKey.make(filePath),
  );
  const candidateTasks = input.tasks.map((task) => {
    if (task.taskId !== input.sourceTaskId) return task;
    const merged = [...new Set([...task.filesAllowed, ...safeUnexpected])];
    if (merged.length === 0) return task;
    const [firstAllowed, ...restAllowed] = merged;
    return {
      ...task,
      filesAllowed: [firstAllowed!, ...restAllowed] as [
        WorkflowBoardNonGitArtifactKey,
        ...WorkflowBoardNonGitArtifactKey[],
      ],
    };
  });
  if (!isValidTaskContractsArray(candidateTasks)) {
    return { ok: false, reason: "contract_conflict" };
  }
  return { ok: true, tasks: candidateTasks };
}

export function parseArgvTokens(raw: string): string[] {
  const tokens: string[] = [];
  let current = "";
  let quote: "'" | '"' | null = null;
  let escaping = false;

  for (const ch of raw.trim()) {
    if (escaping) {
      current += ch;
      escaping = false;
      continue;
    }
    if (ch === "\\" && quote !== "'") {
      escaping = true;
      continue;
    }
    if (quote !== null) {
      if (ch === quote) {
        quote = null;
      } else {
        current += ch;
      }
      continue;
    }
    if (ch === "'" || ch === '"') {
      quote = ch;
      continue;
    }
    if (/\s/.test(ch)) {
      if (current.length > 0) {
        tokens.push(current);
        current = "";
      }
      continue;
    }
    current += ch;
  }
  if (escaping) {
    current += "\\";
  }
  if (current.length > 0) {
    tokens.push(current);
  }
  return tokens;
}

export function formatArgvTokens(argv: ReadonlyArray<string>): string {
  return argv
    .map((token) =>
      /[\s"']/.test(token) ? `"${token.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"` : token,
    )
    .join(" ");
}

export function isPlanOnlyEditPreservingSpecApproval(
  card: Pick<WorkflowCardDetail, "specApproval">,
): boolean {
  return card.specApproval?.status === "valid";
}

export type WorkflowBoardCrossColumnTransition =
  | { readonly kind: "approve_spec"; readonly targetStage: "plan" }
  | { readonly kind: "approve_plan"; readonly targetStage: "execute" }
  | { readonly kind: "approve_delivery"; readonly targetStage: "ship" }
  | {
      readonly kind: "return_for_revision";
      readonly targetStage: WorkflowBoardReturnTargetStage;
    };

export function resolveCrossColumnTransition(
  card: Pick<WorkflowBoardCardSummary, "stage" | "status">,
  targetStage: WorkflowBoardStage,
): WorkflowBoardCrossColumnTransition | null {
  if (card.status === "completed" || card.stage === targetStage) {
    return null;
  }
  if (card.stage === "spec" && targetStage === "plan") {
    return { kind: "approve_spec", targetStage: "plan" };
  }
  if (card.stage === "plan" && targetStage === "execute") {
    return { kind: "approve_plan", targetStage: "execute" };
  }
  if (card.stage === "execute" && targetStage === "ship") {
    return { kind: "approve_delivery", targetStage: "ship" };
  }
  if (
    (card.stage === "plan" && targetStage === "spec") ||
    (card.stage === "execute" && (targetStage === "spec" || targetStage === "plan")) ||
    (card.stage === "ship" &&
      (targetStage === "spec" || targetStage === "plan" || targetStage === "execute"))
  ) {
    return { kind: "return_for_revision", targetStage };
  }
  return null;
}

export function isManualChallengeFallbackValid(input: {
  readonly acknowledgementToken: string;
  readonly answers: readonly [string, string, string];
  readonly unresolvedConcernCount: number;
}): boolean {
  return (
    input.acknowledgementToken === "I acknowledge" &&
    input.unresolvedConcernCount === 0 &&
    input.answers.every((answer) => answer.trim().length > 0)
  );
}

export function parseSpecListLines(text: string): ReadonlyArray<string> {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

export function parseAcceptanceCriteriaLines(
  text: string,
): ReadonlyArray<{ readonly id: string; readonly text: string }> {
  const lines = parseSpecListLines(text);
  return lines.map((line, index) => {
    const match = /^(AC-[A-Za-z0-9_-]+)\s*:\s*(.+)$/.exec(line);
    if (match?.[1] && match[2]) {
      return { id: match[1], text: match[2].trim() };
    }
    return { id: `AC-${index + 1}`, text: line };
  });
}

export function formatAcceptanceCriteriaLines(
  criteria: ReadonlyArray<{ readonly id: string; readonly text: string }>,
): string {
  return criteria.map((item) => `${item.id}: ${item.text}`).join("\n");
}

function buildTaskStatusLookup(
  tasks: ReadonlyArray<WorkflowBoardTaskContract>,
  executions: ReadonlyArray<WorkflowBoardTaskExecutionState>,
): ReadonlyMap<string, WorkflowBoardTaskStatus> {
  const statuses = new Map<string, WorkflowBoardTaskStatus>();
  for (const task of tasks) {
    statuses.set(task.taskId, task.status ?? "todo");
  }
  for (const exec of executions) {
    statuses.set(exec.taskId, exec.status);
  }
  return statuses;
}

export function taskDependencyIds(
  task: WorkflowBoardTaskContract,
  allTasks: ReadonlyArray<WorkflowBoardTaskContract>,
): ReadonlyArray<string> {
  const deps = new Set<string>([...(task.dependsOn ?? []), ...(task.blockedBy ?? [])]);
  for (const other of allTasks) {
    if (other.blocks?.includes(task.taskId)) {
      deps.add(other.taskId);
    }
  }
  return [...deps];
}

export function isTaskDagReady(
  tasks: ReadonlyArray<WorkflowBoardTaskContract>,
  executions: ReadonlyArray<WorkflowBoardTaskExecutionState>,
  taskId: string,
): boolean {
  const task = tasks.find((candidate) => candidate.taskId === taskId);
  if (!task) return false;
  const statuses = buildTaskStatusLookup(tasks, executions);
  for (const depId of taskDependencyIds(task, tasks)) {
    const depStatus = statuses.get(depId);
    if (depStatus !== "completed" && depStatus !== "skipped") {
      return false;
    }
  }
  return true;
}

export function findInProgressTask(
  tasks: ReadonlyArray<WorkflowBoardTaskContract>,
  executions: ReadonlyArray<WorkflowBoardTaskExecutionState>,
): string | null {
  const statuses = buildTaskStatusLookup(tasks, executions);
  for (const [taskId, status] of statuses) {
    if (status === "in_progress") {
      return taskId;
    }
  }
  return null;
}

export function needsCardReconciliation(
  card: Pick<
    WorkflowCardDetail,
    "incompleteJournalCount" | "hasUnreconciledEdits" | "recoveryState"
  >,
): boolean {
  return workflowBoardNeedsRecovery(card);
}

export function unacknowledgedBlockingCliPolicies(
  card: Pick<WorkflowCardDetail, "cliImport"> | null | undefined,
) {
  return workflowBoardUnacknowledgedBlockingPolicies(card);
}

export function canStartExecuteTask(
  input: {
    readonly card: Pick<
      WorkflowCardDetail,
      | "stage"
      | "status"
      | "planApproval"
      | "planApprovalStatus"
      | "fileContractReports"
      | "incompleteJournalCount"
      | "hasUnreconciledEdits"
      | "recoveryState"
      | "cliImport"
    >;
    readonly taskId: string;
    readonly tasks: ReadonlyArray<WorkflowBoardTaskContract>;
    readonly executions: ReadonlyArray<WorkflowBoardTaskExecutionState>;
    readonly canStartStage: boolean;
    readonly workflowsEnabled: boolean;
    readonly workspaceStrategyValid: boolean;
    readonly hasModelSelection: boolean;
    readonly unsupportedPoliciesAcknowledged?: boolean;
  },
  resolve: MessageResolver = m,
): { readonly allowed: boolean; readonly reason: string | null } {
  if (!input.canStartStage) {
    return {
      allowed: false,
      reason: resolve(msg("Orchestration operate permission is required.")),
    };
  }
  if (!input.workflowsEnabled) {
    return { allowed: false, reason: resolve(msg("Workflows are disabled in Settings -> Labs.")) };
  }
  if (!input.workspaceStrategyValid || !input.hasModelSelection) {
    return {
      allowed: false,
      reason: resolve(msg("Select a valid provider, model, and workspace strategy.")),
    };
  }
  if (input.card.stage !== "execute") {
    return { allowed: false, reason: resolve(msg("Card must be in Execute to run tasks.")) };
  }
  if (input.card.status === "paused") {
    return {
      allowed: false,
      reason: resolve(msg("Resume the paused card before starting tasks.")),
    };
  }
  if (input.card.status === "completed") {
    return { allowed: false, reason: resolve(msg("Completed cards cannot start new tasks.")) };
  }
  const planApproved =
    input.card.planApproval?.status === "valid" ||
    input.card.planApprovalStatus === "valid" ||
    (input.card.cliImport !== undefined &&
      (input.card.planApproval?.status === "historical" ||
        input.card.planApprovalStatus === "historical"));
  if (!planApproved) {
    return {
      allowed: false,
      reason: resolve(msg("Plan approval is required or stale. Re-approve Plan first.")),
    };
  }
  if (
    unacknowledgedBlockingCliPolicies(input.card).length > 0 &&
    input.unsupportedPoliciesAcknowledged !== true
  ) {
    return {
      allowed: false,
      reason: resolve(msg("Acknowledge unsupported CLI policies before starting Execute.")),
    };
  }
  if (needsCardReconciliation(input.card)) {
    return {
      allowed: false,
      reason: resolve(msg("Reconcile pending journal entries or external edits first.")),
    };
  }
  if (
    (input.card.fileContractReports ?? []).some((report) => report.status === "violation_pending")
  ) {
    return {
      allowed: false,
      reason: resolve(
        msg("Resolve pending file-contract violations before starting another task."),
      ),
    };
  }
  const inProgressTaskId = findInProgressTask(input.tasks, input.executions);
  if (inProgressTaskId !== null && inProgressTaskId !== input.taskId) {
    return {
      allowed: false,
      reason: resolve(
        msg("Only one mutating task can run at a time ({taskId} is active).", {
          taskId: inProgressTaskId,
        }),
      ),
    };
  }
  const taskStatus = buildTaskStatusLookup(input.tasks, input.executions).get(input.taskId);
  if (taskStatus === "completed" || taskStatus === "skipped") {
    return { allowed: false, reason: resolve(msg("Task is already complete.")) };
  }
  if (taskStatus === "blocked") {
    return {
      allowed: false,
      reason: resolve(msg("Retry this blocked task before starting execution.")),
    };
  }
  if (!isTaskDagReady(input.tasks, input.executions, input.taskId)) {
    return { allowed: false, reason: resolve(msg("Waiting on incomplete prerequisite tasks.")) };
  }
  return { allowed: true, reason: null };
}

export function canStartIndependentReview(
  input: {
    readonly card: Pick<
      WorkflowCardDetail,
      | "stage"
      | "status"
      | "planApproval"
      | "planApprovalStatus"
      | "fileContractReports"
      | "incompleteJournalCount"
      | "hasUnreconciledEdits"
      | "recoveryState"
    >;
    readonly tasks: ReadonlyArray<WorkflowBoardTaskContract>;
    readonly executions: ReadonlyArray<WorkflowBoardTaskExecutionState>;
    readonly canStartStage: boolean;
    readonly workflowsEnabled: boolean;
    readonly workspaceStrategyValid: boolean;
    readonly hasModelSelection: boolean;
  },
  resolve: MessageResolver = m,
): { readonly allowed: boolean; readonly reason: string | null } {
  if (!input.canStartStage) {
    return {
      allowed: false,
      reason: resolve(msg("Orchestration operate permission is required.")),
    };
  }
  if (!input.workflowsEnabled) {
    return { allowed: false, reason: resolve(msg("Workflows are disabled in Settings -> Labs.")) };
  }
  if (!input.workspaceStrategyValid || !input.hasModelSelection) {
    return {
      allowed: false,
      reason: resolve(msg("Select a valid provider, model, and workspace strategy.")),
    };
  }
  if (input.card.stage !== "execute" && input.card.stage !== "ship") {
    return {
      allowed: false,
      reason: resolve(msg("Independent review runs in Execute or Ship.")),
    };
  }
  if (input.card.status === "paused") {
    return {
      allowed: false,
      reason: resolve(msg("Resume the paused card before starting review.")),
    };
  }
  if (input.card.status === "completed") {
    return {
      allowed: false,
      reason: resolve(msg("Completed cards cannot start a new review.")),
    };
  }
  const planApproved =
    input.card.planApproval?.status === "valid" || input.card.planApprovalStatus === "valid";
  if (!planApproved) {
    return {
      allowed: false,
      reason: resolve(msg("Plan approval is required or stale before independent review.")),
    };
  }
  if (needsCardReconciliation(input.card)) {
    return {
      allowed: false,
      reason: resolve(msg("Reconcile pending journal entries or external edits first.")),
    };
  }
  if (
    (input.card.fileContractReports ?? []).some((report) => report.status === "violation_pending")
  ) {
    return {
      allowed: false,
      reason: resolve(msg("Resolve pending file-contract violations before review.")),
    };
  }
  const inProgressTaskId = findInProgressTask(input.tasks, input.executions);
  if (inProgressTaskId !== null) {
    return {
      allowed: false,
      reason: resolve(
        msg("Finish active task {taskId} before launching independent review.", {
          taskId: inProgressTaskId,
        }),
      ),
    };
  }
  return { allowed: true, reason: null };
}

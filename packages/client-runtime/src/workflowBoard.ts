import {
  type ServerSettings,
  type WorkflowBoardAcEvidenceMapping,
  WorkflowBoardAcEvidenceMappingsArray,
  type WorkflowBoardAcEvidenceStatus,
  type WorkflowBoardAcObservationRecord,
  type WorkflowBoardApprovalStatus,
  WorkflowBoardArgvCommand,
  type WorkflowBoardCardStatus,
  type WorkflowBoardCardSummary,
  type WorkflowBoardCheckStatus,
  type WorkflowBoardCliImportHistoricalReceiptKind,
  type WorkflowBoardCliImportPolicyStatus,
  type WorkflowBoardCliImportUnsupportedPolicy,
  type WorkflowBoardDeliveryReceiptStatus,
  type WorkflowBoardDeliveryStatus,
  type WorkflowBoardExternalEditRecord,
  type WorkflowBoardFixPointer,
  type WorkflowBoardPlanAlternative,
  WorkflowBoardPlanAlternativesPair,
  type WorkflowBoardPlanLens,
  type WorkflowBoardPlanLensEntry,
  WorkflowBoardPlanLensesArray,
  type WorkflowBoardPlannedCheck,
  type WorkflowBoardPlannedCheckReceipt,
  type WorkflowBoardPlanningStrategy,
  type WorkflowBoardRecoveryEntry,
  type WorkflowBoardRecoveryState,
  WorkflowBoardRequestError,
  type WorkflowBoardReturnTargetStage,
  type WorkflowBoardReviewFinding,
  type WorkflowBoardReviewReport,
  type WorkflowBoardReviewVerdict,
  WorkflowBoardSafeInputPath,
  type WorkflowBoardStage,
  type WorkflowBoardStageRunLink,
  type WorkflowBoardStageRunStage,
  type WorkflowBoardStrategyMetadata,
  type WorkflowBoardTaskContract,
  WorkflowBoardTaskContractsArray,
  type WorkflowBoardTaskGranularity,
  type WorkflowBoardTaskStatus,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Schema from "effect/Schema";
import { mFallback, msg } from "gt-i18n";
import type { MFunctionType } from "gt-i18n/types";

import { errorDisplayMessage } from "./errors/translatable.ts";

export const WORKFLOW_BOARD_STAGES: readonly WorkflowBoardStage[] = [
  "spec",
  "plan",
  "execute",
  "ship",
];

export type WorkflowBoardAvailability =
  | "enabled"
  | "loading"
  | "disconnected"
  | "unsupported"
  | "labs-disabled";

export interface WorkflowBoardGatingInput {
  readonly settings?:
    | (Pick<ServerSettings, "workflowsEnabled"> & {
        readonly workflowsEnabled?: boolean | undefined;
      })
    | null
    | undefined;
  readonly capabilities?:
    | { readonly projectWorkflowBoard?: boolean | undefined }
    | null
    | undefined;
}

export function isProjectWorkflowBoardSupported(
  capabilities?: WorkflowBoardGatingInput["capabilities"],
): boolean {
  return capabilities?.projectWorkflowBoard === true;
}

export function isProjectWorkflowBoardEnabled(input: WorkflowBoardGatingInput): boolean {
  return (
    input.settings?.workflowsEnabled === true && isProjectWorkflowBoardSupported(input.capabilities)
  );
}

export function projectWorkflowBoardAvailability(
  input: WorkflowBoardGatingInput & {
    readonly connected: boolean;
    readonly configLoaded?: boolean | undefined;
  },
): WorkflowBoardAvailability {
  if (!input.connected) return "disconnected";
  if (input.configLoaded === false) return "loading";
  if (!isProjectWorkflowBoardSupported(input.capabilities)) return "unsupported";
  if (input.settings?.workflowsEnabled !== true) return "labs-disabled";
  return "enabled";
}

export interface WorkflowBoardAvailabilityNotice {
  readonly title: string;
  readonly description: string;
  readonly action?: "open-labs-settings";
}

export function workflowBoardAvailabilityNotice(
  availability: WorkflowBoardAvailability,
  resolve: MFunctionType = mFallback,
): WorkflowBoardAvailabilityNotice | null {
  switch (availability) {
    case "enabled":
      return null;
    case "loading":
      return {
        title: resolve(msg("Loading workflow board…")),
        description: resolve(msg("Checking server workflow board capabilities.")),
      };
    case "disconnected":
      return {
        title: resolve(msg("Environment is not connected")),
        description: resolve(msg("Connect this environment to open its project workflow board.")),
      };
    case "unsupported":
      return {
        title: resolve(msg("Workflow board is not supported")),
        description: resolve(
          msg("This server does not advertise project workflow board support yet."),
        ),
      };
    case "labs-disabled":
      return {
        title: resolve(msg("Workflows are off")),
        description: resolve(
          msg("Enable Workflows in Settings → Labs to use the project workflow board."),
        ),
        action: "open-labs-settings",
      };
  }
}

export function workflowBoardReadPermissionNotice(
  resolve: MFunctionType = mFallback,
): WorkflowBoardAvailabilityNotice {
  return {
    title: resolve(msg("Read permission required")),
    description: resolve(msg("This session does not have orchestration read access.")),
  };
}

export function workflowBoardProjectNotFoundNotice(
  resolve: MFunctionType = mFallback,
): WorkflowBoardAvailabilityNotice {
  return {
    title: resolve(msg("Project not found")),
    description: resolve(msg("Choose a connected project to open its workflow board.")),
  };
}

export interface WorkflowBoardColumns {
  readonly spec: ReadonlyArray<WorkflowBoardCardSummary>;
  readonly plan: ReadonlyArray<WorkflowBoardCardSummary>;
  readonly execute: ReadonlyArray<WorkflowBoardCardSummary>;
  readonly ship: ReadonlyArray<WorkflowBoardCardSummary>;
  readonly completed: ReadonlyArray<WorkflowBoardCardSummary>;
}

function compareIsoTimestamps(left: string, right: string): number {
  const leftTime = Date.parse(left);
  const rightTime = Date.parse(right);
  return Number.isFinite(leftTime) && Number.isFinite(rightTime)
    ? leftTime - rightTime
    : left.localeCompare(right);
}

function compareCardsDeterministic(
  left: WorkflowBoardCardSummary,
  right: WorkflowBoardCardSummary,
): number {
  const updatedOrder = compareIsoTimestamps(right.updatedAt, left.updatedAt);
  if (updatedOrder !== 0) return updatedOrder;
  return left.cardId.localeCompare(right.cardId);
}

/**
 * Group compact card summaries into the four stage columns plus a completed shelf.
 * Blocked and paused cards remain visible in their current stage column.
 */
export function groupWorkflowBoardCards(
  cards: ReadonlyArray<WorkflowBoardCardSummary>,
): WorkflowBoardColumns {
  const spec: WorkflowBoardCardSummary[] = [];
  const plan: WorkflowBoardCardSummary[] = [];
  const execute: WorkflowBoardCardSummary[] = [];
  const ship: WorkflowBoardCardSummary[] = [];
  const completed: WorkflowBoardCardSummary[] = [];

  for (const card of cards) {
    if (card.status === "completed") {
      completed.push(card);
      continue;
    }
    switch (card.stage) {
      case "spec":
        spec.push(card);
        break;
      case "plan":
        plan.push(card);
        break;
      case "execute":
        execute.push(card);
        break;
      case "ship":
        ship.push(card);
        break;
    }
  }

  spec.sort(compareCardsDeterministic);
  plan.sort(compareCardsDeterministic);
  execute.sort(compareCardsDeterministic);
  ship.sort(compareCardsDeterministic);
  completed.sort(compareCardsDeterministic);

  return { spec, plan, execute, ship, completed };
}

export function workflowBoardStageLabel(
  stage: WorkflowBoardStage,
  resolve: MFunctionType = mFallback,
): string {
  switch (stage) {
    case "spec":
      return resolve(msg("Spec", { $context: "workflow board stage tab and column" }));
    case "plan":
      return resolve(msg("Plan", { $context: "workflow board stage tab and column" }));
    case "execute":
      return resolve(msg("Execute", { $context: "workflow board stage tab and column" }));
    case "ship":
      return resolve(msg("Ship", { $context: "workflow board stage tab and column" }));
  }
}

export function workflowBoardCardStatusLabel(
  status: WorkflowBoardCardStatus,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "active":
      return resolve(msg("Active", { $context: "workflow board card status" }));
    case "blocked":
      return resolve(msg("Blocked", { $context: "workflow board card status" }));
    case "paused":
      return resolve(msg("Paused", { $context: "workflow board card status" }));
    case "completed":
      return resolve(msg("Completed", { $context: "workflow board card status" }));
  }
}

export type WorkflowBoardSpecApprovalState = "approved" | "stale" | "unapproved";

export function workflowBoardSpecApprovalState(
  card: Pick<WorkflowBoardCardSummary, "specApprovalStatus">,
): WorkflowBoardSpecApprovalState {
  if (card.specApprovalStatus === "valid") return "approved";
  if (card.specApprovalStatus === "stale") return "stale";
  return "unapproved";
}

export function workflowBoardSpecApprovalLabel(
  status: WorkflowBoardApprovalStatus | null,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "valid":
      return resolve(msg("Spec approved", { $context: "workflow board spec approval state" }));
    case "stale":
      return resolve(msg("Approval stale", { $context: "workflow board spec approval state" }));
    case "superseded":
      return resolve(msg("Superseded", { $context: "workflow board spec approval state" }));
    case "historical":
      return resolve(msg("Historical", { $context: "workflow board spec approval state" }));
    case null:
      return resolve(msg("Spec draft", { $context: "workflow board spec approval state" }));
  }
}

export function workflowBoardPlanApprovalState(
  card: Pick<WorkflowBoardCardSummary, "planApprovalStatus">,
): WorkflowBoardSpecApprovalState {
  if (card.planApprovalStatus === "valid") return "approved";
  if (card.planApprovalStatus === "stale") return "stale";
  return "unapproved";
}

export function workflowBoardPlanApprovalLabel(
  status: WorkflowBoardApprovalStatus | null | undefined,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "valid":
      return resolve(msg("Plan approved", { $context: "workflow board plan approval state" }));
    case "stale":
      return resolve(
        msg("Plan approval stale", { $context: "workflow board plan approval state" }),
      );
    case "superseded":
      return resolve(msg("Superseded", { $context: "workflow board plan approval state" }));
    case "historical":
      return resolve(msg("Historical", { $context: "workflow board plan approval state" }));
    case null:
    case undefined:
      return resolve(msg("Plan draft", { $context: "workflow board plan approval state" }));
  }
}

export function workflowBoardCardBlocker(
  card: Pick<WorkflowBoardCardSummary, "status" | "blockerSummary">,
  resolve: MFunctionType = mFallback,
): string | null {
  const summary = card.blockerSummary?.trim() ?? "";
  if (summary !== "") return summary;
  if (card.status === "blocked") {
    return resolve(msg("Blocked", { $context: "workflow board fallback card blocker" }));
  }
  return null;
}

export function workflowBoardCardNextAction(
  card: Pick<
    WorkflowBoardCardSummary,
    "stage" | "status" | "blockerSummary" | "nextAction" | "specApprovalStatus" | "activeThreadId"
  >,
  resolve: MFunctionType = mFallback,
): string {
  if (card.status === "blocked") {
    return (
      workflowBoardCardBlocker(card, resolve) ??
      resolve(msg("Resolve blocker", { $context: "workflow board card next action" }))
    );
  }
  if (card.status === "paused") {
    return resolve(msg("Resume card", { $context: "workflow board card next action" }));
  }
  if (card.status === "completed") {
    return resolve(msg("View completed card", { $context: "workflow board card next action" }));
  }
  switch (card.nextAction) {
    case "draft_spec":
      return resolve(
        msg("Draft and approve spec", { $context: "workflow board card next action" }),
      );
    case "approve_spec":
      return resolve(msg("Approve spec", { $context: "workflow board card next action" }));
    case "explore_spec":
      return resolve(
        msg("Continue guided exploration", { $context: "workflow board card next action" }),
      );
    case "resume_exploration":
      return resolve(msg("Resume exploration", { $context: "workflow board card next action" }));
    case "author_spec":
      return resolve(msg("Open Spec thread", { $context: "workflow board card next action" }));
    case "start_plan":
      return resolve(msg("Start Plan", { $context: "workflow board card next action" }));
    case "author_plan":
    case "view_plan_thread":
      return resolve(msg("Open Plan thread", { $context: "workflow board card next action" }));
    case "revise_spec":
      return resolve(msg("Revise spec", { $context: "workflow board card next action" }));
    case "revise_plan":
      return resolve(msg("Revise plan", { $context: "workflow board card next action" }));
    case "revise_execute":
      return resolve(msg("Revise execution", { $context: "workflow board card next action" }));
    case "start_execute":
      return resolve(msg("Ready for execution", { $context: "workflow board card next action" }));
    case "complete_task":
      return resolve(msg("Complete active task", { $context: "workflow board card next action" }));
    case "run_checks_or_next_task":
      return resolve(
        msg("Run planned checks or next task", { $context: "workflow board card next action" }),
      );
    case "continue_execute":
    case "continue_stage":
      return resolve(msg("Continue execution", { $context: "workflow board card next action" }));
    case "run_review":
      return resolve(
        msg("Start independent review", { $context: "workflow board card next action" }),
      );
    case "await_review":
      return resolve(msg("Open Review thread", { $context: "workflow board card next action" }));
    case "ready_for_ship":
      return resolve(msg("Review for delivery", { $context: "workflow board card next action" }));
    case "resolve_contract_violation":
      return resolve(
        msg("Resolve file-contract violation", { $context: "workflow board card next action" }),
      );
    case "reconcile_external_action":
      return resolve(
        msg("Reconcile external changes", { $context: "workflow board card next action" }),
      );
    case "retry_check":
      return resolve(msg("Retry failed check", { $context: "workflow board card next action" }));
    case "retry_review":
      return resolve(
        msg("Retry independent review", { $context: "workflow board card next action" }),
      );
    case "retry_task":
      return resolve(msg("Retry failed task", { $context: "workflow board card next action" }));
    case "retry_stage":
    case "retry_step":
      return resolve(msg("Retry failed step", { $context: "workflow board card next action" }));
    case "resolve_finding":
      return resolve(
        msg("Resolve review findings", { $context: "workflow board card next action" }),
      );
    case "fix_finding":
      return resolve(msg("Fix review finding", { $context: "workflow board card next action" }));
    case "resume_card":
      return resolve(msg("Resume card", { $context: "workflow board card next action" }));
    case "git_actions":
      return resolve(
        msg("Select Git delivery actions", { $context: "workflow board card next action" }),
      );
    case "acknowledge_policies":
      return resolve(
        msg("Acknowledge imported policies", { $context: "workflow board card next action" }),
      );
    case "resolve_blocked_task":
      return resolve(msg("Resolve blocked task", { $context: "workflow board card next action" }));
    default: {
      const raw = card.nextAction?.trim() ?? "";
      if (raw !== "") return raw;
      if (card.stage === "spec") {
        return card.specApprovalStatus === "valid"
          ? resolve(msg("Start Plan", { $context: "workflow board card next action" }))
          : resolve(msg("Draft and approve spec", { $context: "workflow board card next action" }));
      }
      if (card.stage === "plan") {
        if (card.activeThreadId !== null) {
          return resolve(msg("Open Plan thread", { $context: "workflow board card next action" }));
        }
        return card.specApprovalStatus === "valid"
          ? resolve(msg("Start Plan", { $context: "workflow board card next action" }))
          : resolve(msg("Re-approve spec", { $context: "workflow board card next action" }));
      }
      if (card.stage === "execute") {
        return resolve(msg("Follow execution", { $context: "workflow board card next action" }));
      }
      return resolve(msg("Review for delivery", { $context: "workflow board card next action" }));
    }
  }
}

export function workflowBoardLatestStageRun(
  detail: Pick<WorkflowCardDetail, "stageRuns">,
  stage: WorkflowBoardStageRunStage,
): WorkflowBoardStageRunLink | null {
  let latest: WorkflowBoardStageRunLink | null = null;
  for (const run of detail.stageRuns) {
    if (run.stage !== stage) continue;
    const order = latest === null ? 1 : compareIsoTimestamps(run.updatedAt, latest.updatedAt);
    if (latest === null || order > 0 || (order === 0 && run.cardRevision >= latest.cardRevision)) {
      latest = run;
    }
  }
  return latest;
}

export function isWorkflowBoardCardShipped(
  card: Pick<WorkflowBoardCardSummary, "status" | "deliveryStatus">,
): boolean {
  return card.status === "completed" || card.deliveryStatus === "shipped";
}

export function workflowBoardDeliveryStatusLabel(
  status: WorkflowBoardDeliveryStatus | WorkflowBoardDeliveryReceiptStatus | null | undefined,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "shipped":
    case "valid":
      return resolve(msg("Shipped", { $context: "workflow board delivery status" }));
    case "stale":
    case "historical":
      return workflowBoardCheckStatusLabel(status, resolve);
    case "superseded":
      return workflowBoardSpecApprovalLabel(status, resolve);
    case "ready":
      return resolve(msg("Ready to ship", { $context: "workflow board delivery status" }));
    case "blocked":
      return resolve(msg("Ship blocked", { $context: "workflow board delivery status" }));
    case "pending":
    case null:
    case undefined:
      return resolve(msg("Delivery pending", { $context: "workflow board delivery status" }));
  }
}

export function workflowBoardCheckStatusLabel(
  status: WorkflowBoardCheckStatus,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "pending":
      return resolve(msg("Pending", { $context: "workflow board planned check status" }));
    case "running":
      return resolve(msg("Running", { $context: "workflow board planned check status" }));
    case "passed":
      return resolve(msg("Passed", { $context: "workflow board planned check status" }));
    case "failed":
      return resolve(msg("Failed", { $context: "workflow board planned check status" }));
    case "stale":
      return resolve(msg("Stale", { $context: "workflow board planned check status" }));
    case "historical":
      return resolve(msg("Historical", { $context: "workflow board planned check status" }));
  }
}

export function workflowBoardReviewVerdictLabel(
  verdict: WorkflowBoardReviewVerdict | null | undefined,
  resolve: MFunctionType = mFallback,
): string {
  switch (verdict) {
    case "pass":
      return resolve(msg("Pass", { $context: "workflow board review verdict" }));
    case "findings":
      return resolve(msg("Findings", { $context: "workflow board review verdict" }));
    case "failed":
      return resolve(msg("Failed", { $context: "workflow board review verdict" }));
    case "stale":
      return resolve(msg("Stale", { $context: "workflow board review verdict" }));
    case "pending":
    case null:
    case undefined:
      return resolve(msg("Pending", { $context: "workflow board review verdict" }));
  }
}

export function workflowBoardAcEvidenceStatusLabel(
  status: WorkflowBoardAcEvidenceStatus,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "pending":
      return resolve(msg("Pending", { $context: "workflow board acceptance evidence status" }));
    case "pass":
      return resolve(msg("Pass", { $context: "workflow board acceptance evidence status" }));
    case "fail":
      return resolve(msg("Fail", { $context: "workflow board acceptance evidence status" }));
    case "not_applicable":
      return resolve(
        msg("Not applicable", { $context: "workflow board acceptance evidence status" }),
      );
    case "stale":
      return resolve(msg("Stale", { $context: "workflow board acceptance evidence status" }));
    case "historical":
      return resolve(msg("Historical", { $context: "workflow board acceptance evidence status" }));
  }
}

export function workflowBoardNeedsRecovery(
  card: Pick<WorkflowBoardCardSummary, "incompleteJournalCount" | "hasUnreconciledEdits"> & {
    readonly recoveryState?: WorkflowBoardRecoveryState | null | undefined;
  },
): boolean {
  if (card.recoveryState?.requiresReconciliation) return true;
  if ((card.incompleteJournalCount ?? 0) > 0) return true;
  if (card.hasUnreconciledEdits === true) return true;
  if ((card.recoveryState?.incompleteJournalCount ?? 0) > 0) return true;
  return card.recoveryState?.externalEdits.some((edit) => edit.status === "detected") ?? false;
}

export interface WorkflowBoardRecoverySummary {
  readonly requiresReconciliation: boolean;
  readonly incompleteJournalCount: number;
  readonly pendingJournalEntries: ReadonlyArray<WorkflowBoardRecoveryEntry>;
  readonly detectedExternalEdits: ReadonlyArray<WorkflowBoardExternalEditRecord>;
}

export function workflowBoardRecoverySummary(
  detail: Pick<
    WorkflowCardDetail,
    "incompleteJournalCount" | "hasUnreconciledEdits" | "recoveryState"
  >,
): WorkflowBoardRecoverySummary {
  const pendingJournalEntries = detail.recoveryState?.pendingJournalEntries ?? [];
  const detectedExternalEdits =
    detail.recoveryState?.externalEdits.filter((edit) => edit.status === "detected") ?? [];
  return {
    requiresReconciliation: workflowBoardNeedsRecovery(detail),
    incompleteJournalCount:
      detail.recoveryState?.incompleteJournalCount ??
      detail.incompleteJournalCount ??
      pendingJournalEntries.length,
    pendingJournalEntries,
    detectedExternalEdits,
  };
}

function detailTaskContracts(
  detail: Pick<WorkflowCardDetail, "plan" | "tasks">,
): ReadonlyArray<WorkflowBoardTaskContract> {
  return detail.plan?.tasks ?? detail.tasks ?? [];
}

export function workflowBoardTaskStatusMap(
  detail: Pick<WorkflowCardDetail, "plan" | "tasks" | "taskExecutions">,
): ReadonlyMap<string, WorkflowBoardTaskStatus> {
  const statuses = new Map<string, WorkflowBoardTaskStatus>();
  for (const task of detailTaskContracts(detail)) {
    statuses.set(task.taskId, task.status ?? "todo");
  }
  for (const exec of detail.taskExecutions ?? []) {
    statuses.set(exec.taskId, exec.status);
  }
  return statuses;
}

export function workflowBoardActiveTask(
  detail: Pick<WorkflowCardDetail, "plan" | "tasks" | "taskExecutions">,
): WorkflowBoardTaskContract | null {
  const statuses = workflowBoardTaskStatusMap(detail);
  for (const task of detailTaskContracts(detail)) {
    if (statuses.get(task.taskId) === "in_progress") {
      return task;
    }
  }
  return null;
}

export function workflowBoardIsTaskDagReady(
  detail: Pick<WorkflowCardDetail, "plan" | "tasks" | "taskExecutions">,
  taskId: string,
): boolean {
  const tasks = detailTaskContracts(detail);
  const task = tasks.find((candidate) => candidate.taskId === taskId);
  if (!task) return false;
  const statuses = workflowBoardTaskStatusMap(detail);
  const dependencies = new Set<string>([...(task.dependsOn ?? []), ...(task.blockedBy ?? [])]);
  for (const other of tasks) {
    if (other.blocks?.includes(taskId)) {
      dependencies.add(other.taskId);
    }
  }
  for (const depId of dependencies) {
    const depStatus = statuses.get(depId);
    if (depStatus !== "completed" && depStatus !== "skipped") {
      return false;
    }
  }
  return true;
}

export function workflowBoardNextReadyTask(
  detail: Pick<
    WorkflowCardDetail,
    | "status"
    | "plan"
    | "tasks"
    | "taskExecutions"
    | "fileContractReports"
    | "incompleteJournalCount"
    | "hasUnreconciledEdits"
    | "recoveryState"
  >,
): WorkflowBoardTaskContract | null {
  if (detail.status === "paused" || detail.status === "completed") return null;
  if (workflowBoardNeedsRecovery(detail)) return null;
  if ((detail.fileContractReports ?? []).some((report) => report.status === "violation_pending")) {
    return null;
  }
  const statuses = workflowBoardTaskStatusMap(detail);
  for (const status of statuses.values()) {
    if (status === "in_progress") return null;
  }
  const tasks = detailTaskContracts(detail);
  for (const task of tasks) {
    const status = statuses.get(task.taskId) ?? "todo";
    if (
      (status === "todo" || status === "pending") &&
      workflowBoardIsTaskDagReady(detail, task.taskId)
    ) {
      return task;
    }
  }
  for (const task of tasks) {
    if (
      statuses.get(task.taskId) === "failed" &&
      workflowBoardIsTaskDagReady(detail, task.taskId)
    ) {
      return task;
    }
  }
  return null;
}

export function workflowBoardUnresolvedFindings(
  detail: Pick<WorkflowCardDetail, "reviewReport">,
): ReadonlyArray<WorkflowBoardReviewFinding> {
  const report = detail.reviewReport;
  if (!report || report.status === "stale" || report.status === "historical") {
    return [];
  }
  return report.findings.filter(
    (finding) =>
      finding.status === "open" ||
      (finding.status === undefined && finding.disposition === "unresolved") ||
      (finding.status === "resolved" && finding.disposition === "fix"),
  );
}

export interface WorkflowBoardStaleEvidenceSummary {
  readonly staleChecks: number;
  readonly staleReview: boolean;
  readonly staleAcObservations: number;
  readonly openViolations: number;
  readonly totalStale: number;
}

export function workflowBoardStaleEvidenceSummary(
  detail: Pick<
    WorkflowCardDetail,
    "checkReceipts" | "reviewReport" | "acObservations" | "fileContractReports" | "shipReadiness"
  >,
): WorkflowBoardStaleEvidenceSummary {
  const currentSnapshot = detail.shipReadiness?.productSnapshotDigest ?? null;
  const staleChecks = (detail.checkReceipts ?? []).filter(
    (receipt) =>
      receipt.status === "stale" ||
      receipt.status === "historical" ||
      (currentSnapshot !== null &&
        receipt.status === "passed" &&
        receipt.productSnapshotDigest !== currentSnapshot),
  ).length;
  const report = detail.reviewReport ?? null;
  const staleReview =
    report !== null &&
    (report.status === "stale" ||
      report.status === "historical" ||
      report.verdict === "stale" ||
      (currentSnapshot !== null &&
        report.status === "completed" &&
        report.productSnapshotDigest !== currentSnapshot));
  const staleAcObservations = (detail.acObservations ?? []).filter(
    (obs) =>
      obs.status === "stale" ||
      obs.status === "historical" ||
      (currentSnapshot !== null &&
        (obs.status === "pass" || obs.status === "not_applicable") &&
        obs.productSnapshotDigest !== currentSnapshot),
  ).length;
  const openViolations = (detail.fileContractReports ?? []).filter(
    (reportItem) => reportItem.status === "violation_pending",
  ).length;
  return {
    staleChecks,
    staleReview,
    staleAcObservations,
    openViolations,
    totalStale: staleChecks + (staleReview ? 1 : 0) + staleAcObservations,
  };
}

export function canApproveWorkflowBoardDelivery(input: {
  readonly detail:
    | Pick<WorkflowCardDetail, "status" | "deliveryStatus" | "deliveryReceipt" | "shipReadiness">
    | null
    | undefined;
  readonly canOperate: boolean;
}): boolean {
  const { detail, canOperate } = input;
  if (!canOperate || !detail) return false;
  if (detail.status === "paused" || detail.status === "completed") return false;
  if (detail.deliveryStatus === "shipped" || detail.deliveryReceipt?.status === "shipped") {
    return false;
  }
  return detail.shipReadiness?.ready === true && (detail.shipReadiness.blockers.length ?? 0) === 0;
}

const isSafeCliImportPath = Schema.is(WorkflowBoardSafeInputPath);

export function isValidCliImportDirInput(raw: string): boolean {
  const trimmed = raw.trim();
  if (trimmed === "") return true;
  return isSafeCliImportPath(trimmed);
}

export function workflowBoardCliImportPolicyStatusLabel(
  status: WorkflowBoardCliImportPolicyStatus,
  resolve: MFunctionType = mFallback,
): string {
  switch (status) {
    case "unsupported":
      return resolve(
        msg("Requires acknowledgement", { $context: "workflow board cli import policy status" }),
      );
    case "acknowledged":
      return resolve(msg("Acknowledged", { $context: "workflow board cli import policy status" }));
  }
}

export function workflowBoardCliImportHistoricalReceiptKindLabel(
  kind: WorkflowBoardCliImportHistoricalReceiptKind,
  resolve: MFunctionType = mFallback,
): string {
  switch (kind) {
    case "spec_approval":
      return resolve(
        msg("Spec approval", { $context: "workflow board cli import historical receipt kind" }),
      );
    case "plan_approval":
      return resolve(
        msg("Plan approval", { $context: "workflow board cli import historical receipt kind" }),
      );
    case "execution":
      return resolve(
        msg("Execution", { $context: "workflow board cli import historical receipt kind" }),
      );
    case "review":
      return resolve(
        msg("Review", { $context: "workflow board cli import historical receipt kind" }),
      );
    case "check":
      return resolve(
        msg("Check", { $context: "workflow board cli import historical receipt kind" }),
      );
  }
}

export function workflowBoardUnacknowledgedBlockingPolicies(
  detail: Pick<WorkflowCardDetail, "cliImport"> | null | undefined,
): ReadonlyArray<WorkflowBoardCliImportUnsupportedPolicy> {
  return (detail?.cliImport?.unsupportedPolicies ?? []).filter(
    (policy) => policy.blocksExecution && policy.status !== "acknowledged",
  );
}

export function workflowBoardHasHistoricalEvidence(
  detail:
    | Pick<
        WorkflowCardDetail,
        | "specApprovalStatus"
        | "planApprovalStatus"
        | "checkReceipts"
        | "reviewReport"
        | "acObservations"
        | "cliImport"
      >
    | null
    | undefined,
): boolean {
  if (!detail) return false;
  if ((detail.cliImport?.historicalReceipts.length ?? 0) > 0) return true;
  if (detail.specApprovalStatus === "historical" || detail.planApprovalStatus === "historical") {
    return true;
  }
  if ((detail.checkReceipts ?? []).some((receipt) => receipt.status === "historical")) {
    return true;
  }
  if (detail.reviewReport?.status === "historical") return true;
  return (detail.acObservations ?? []).some((obs) => obs.status === "historical");
}

const isWorkflowBoardRequestError = Schema.is(WorkflowBoardRequestError);

function isUnsupportedWorkflowBoardRpcError(error: unknown): boolean {
  if (typeof error === "string") {
    const lower = error.toLowerCase();
    return (
      lower.includes("unknown request tag: workflowboard.") ||
      lower.includes("unsupported rpc method") ||
      lower.includes("workflow board is handled by the rust server")
    );
  }
  if (typeof error !== "object" || error === null) return false;
  const tag = "_tag" in error && typeof error._tag === "string" ? error._tag : null;
  const name = "name" in error && typeof error.name === "string" ? error.name : null;
  if (tag === "UnsupportedRpcMethod" || tag === "RpcNoSuchMethodError") {
    return true;
  }
  const message =
    "message" in error && typeof error.message === "string" ? error.message.toLowerCase() : "";
  if (tag === "SchemaError" || name === "SchemaError") {
    return (
      message.includes("workflowboardrequesterror") ||
      message.includes("unsupportedrpcmethod") ||
      message.includes("workflowboard.") ||
      message.includes("unknown request tag: workflowboard.") ||
      message.includes("unsupported rpc method") ||
      message.includes("workflow board is handled by the rust server")
    );
  }
  return (
    message.includes("unsupported rpc method") ||
    message.includes("unknown request tag: workflowboard.") ||
    message.includes("unknown method") ||
    message.includes("method not found") ||
    message.includes("workflow board is handled by the rust server")
  );
}

export function workflowBoardErrorMessage(
  error: unknown,
  resolve: MFunctionType = mFallback,
): string | null {
  if (isUnsupportedWorkflowBoardRpcError(error)) {
    return resolve(msg("This server does not support this workflow board action."));
  }
  if (isWorkflowBoardRequestError(error)) {
    const serverMessage = error.message.trim();
    switch (error.code) {
      case "revision_conflict":
        return error.currentRevision === undefined
          ? resolve(msg("This card changed on the server. Refresh and try again."))
          : resolve(
              msg("This card changed on the server (revision {revision}). Refresh and try again.", {
                revision: error.currentRevision,
              }),
            );
      case "spec_not_approved":
        return resolve(msg("Approve the spec before starting Plan."));
      case "plan_not_approved":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("Complete and approve the plan before proceeding."));
      case "exploration_unresolved":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("Resolve all exploration decisions before approving the spec."));
      case "challenge_unresolved":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("Complete or resolve challenge concerns before approving the spec."));
      case "external_edit_detected":
        return serverMessage !== ""
          ? error.message
          : resolve(
              msg("External file edits were detected. Review and refresh before continuing."),
            );
      case "contract_conflict":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("The task file contracts or dependencies have a conflict."));
      case "digest_mismatch":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("Artifacts on disk do not match the expected revision digest."));
      case "card_not_found":
        return resolve(msg("This workflow card could not be found."));
      case "project_not_found":
        return resolve(msg("The project could not be found on this server."));
      case "artifact_not_found":
        return resolve(msg("The requested artifact was not found."));
      case "invalid_artifact_key":
        return resolve(msg("The artifact key is not valid."));
      case "invalid_stage_transition":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("This stage transition is not allowed right now."));
      case "launch_failed":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("Failed to start the stage thread."));
      case "invalid_input":
      case "internal_error":
        return serverMessage !== ""
          ? error.message
          : resolve(msg("The workflow board request failed."));
    }
  }
  return errorDisplayMessage(error, resolve);
}

export const WORKFLOW_BOARD_PLAN_LENSES: readonly WorkflowBoardPlanLens[] = [
  "ux",
  "functional",
  "architecture",
  "quality",
  "infrastructure",
  "delivery",
];

const isSafeArgvCommandSchema = Schema.is(WorkflowBoardArgvCommand);
const isValidPlanLensesArray = Schema.is(WorkflowBoardPlanLensesArray);
const isValidPlanAlternativesPair = Schema.is(WorkflowBoardPlanAlternativesPair);
const isValidTaskContractsArray = Schema.is(WorkflowBoardTaskContractsArray);
const isValidAcEvidenceMappingsArray = Schema.is(WorkflowBoardAcEvidenceMappingsArray);

export function isSafeWorkflowBoardArgv(argv: ReadonlyArray<string>): boolean {
  return isSafeArgvCommandSchema(argv);
}

function normalizeAcEvidenceMapping(
  mapping: WorkflowBoardAcEvidenceMapping,
): WorkflowBoardAcEvidenceMapping {
  const procedure = mapping.procedure?.trim();
  const externalRef = mapping.externalRef?.trim();
  const externalSource = mapping.externalSource?.trim();
  return {
    criterionId: mapping.criterionId.trim(),
    method: mapping.method,
    ...(mapping.checkIds !== undefined ? { checkIds: mapping.checkIds } : {}),
    ...(mapping.command !== undefined && mapping.command.length > 0
      ? { command: mapping.command }
      : {}),
    ...(procedure !== undefined && procedure.length > 0 ? { procedure } : {}),
    ...(externalRef !== undefined && externalRef.length > 0 ? { externalRef } : {}),
    ...(externalSource !== undefined && externalSource.length > 0 ? { externalSource } : {}),
    ...(mapping.limitations !== undefined ? { limitations: mapping.limitations } : {}),
  };
}

export function normalizeAcEvidenceMappings(
  mappings: ReadonlyArray<WorkflowBoardAcEvidenceMapping>,
): ReadonlyArray<WorkflowBoardAcEvidenceMapping> {
  return mappings.map(normalizeAcEvidenceMapping);
}

export function canApproveWorkflowBoardSpec(input: {
  readonly detail: Pick<
    WorkflowCardDetail,
    "spec" | "specApproval" | "exploration" | "challengeReceipt"
  > | null;
  readonly canOperate: boolean;
}): boolean {
  if (
    !input.canOperate ||
    input.detail === null ||
    input.detail.spec.specDigest === null ||
    input.detail.spec.acceptanceCriteria.length === 0 ||
    input.detail.specApproval?.status === "valid"
  ) {
    return false;
  }
  const unresolvedExploration = input.detail.exploration?.unresolvedIds.length ?? 0;
  if (unresolvedExploration > 0) {
    return false;
  }
  const challenge = input.detail.challengeReceipt;
  if (challenge !== null && challenge !== undefined) {
    if (
      challenge.status === "pending" ||
      challenge.status === "in_progress" ||
      challenge.status === "stale"
    ) {
      return false;
    }
    if (challenge.concerns.some((concern) => concern.disposition === "unresolved")) {
      return false;
    }
  }
  return true;
}

export function isPlanStrategyValid(input: {
  readonly strategy: WorkflowBoardPlanningStrategy | null;
  readonly strategyRationale: string;
  readonly strategyMetadata: WorkflowBoardStrategyMetadata | null;
}): boolean {
  if (input.strategy === null) return false;
  if (input.strategy === "custom") {
    return input.strategyRationale.trim().length > 0;
  }
  if (input.strategy === "risk-first") {
    return (
      (input.strategyMetadata?.uncertainty?.trim().length ?? 0) > 0 &&
      (input.strategyMetadata?.probeTaskId?.trim().length ?? 0) > 0
    );
  }
  if (input.strategy === "expand-contract") {
    return (
      (input.strategyMetadata?.compatibilityWindow?.trim().length ?? 0) > 0 &&
      (input.strategyMetadata?.transitionStep?.trim().length ?? 0) > 0 &&
      (input.strategyMetadata?.retirementPrereq?.trim().length ?? 0) > 0
    );
  }
  return true;
}

export type PlanApprovalBlockerCode =
  | "spec_not_approved"
  | "plan_not_saved"
  | "lenses_incomplete"
  | "alternatives_incomplete"
  | "strategy_incomplete"
  | "granularity_missing"
  | "tasks_incomplete"
  | "checks_invalid"
  | "ac_mappings_incomplete";

export function collectPlanApprovalBlockers(
  detail: Pick<
    WorkflowCardDetail,
    | "stage"
    | "spec"
    | "specApproval"
    | "plan"
    | "lenses"
    | "alternatives"
    | "selectedAlternativeId"
    | "selectionRationale"
    | "strategy"
    | "strategyRationale"
    | "strategyMetadata"
    | "granularity"
    | "tasks"
    | "checks"
    | "acMappings"
  >,
): readonly PlanApprovalBlockerCode[] {
  const blockers: PlanApprovalBlockerCode[] = [];
  if (detail.specApproval?.status !== "valid") {
    blockers.push("spec_not_approved");
  }
  const plan = detail.plan ?? null;
  if (plan === null || plan.planDigest === null) {
    blockers.push("plan_not_saved");
  }

  const lenses: ReadonlyArray<WorkflowBoardPlanLensEntry> = plan?.lenses ?? detail.lenses ?? [];
  const allLensesDecided =
    WORKFLOW_BOARD_PLAN_LENSES.every((lens) => {
      const entry = lenses.find((candidate) => candidate.lens === lens);
      return entry !== undefined && entry.applicable !== null;
    }) && isValidPlanLensesArray(lenses);
  if (!allLensesDecided) {
    blockers.push("lenses_incomplete");
  }

  const alternatives: ReadonlyArray<WorkflowBoardPlanAlternative> =
    plan?.alternatives ?? detail.alternatives ?? [];
  const selectedAlternativeId = plan?.selectedAlternativeId ?? detail.selectedAlternativeId ?? null;
  const selectionRationale = plan?.selectionRationale ?? detail.selectionRationale ?? "";
  const validAlternatives =
    isValidPlanAlternativesPair(alternatives) &&
    selectedAlternativeId !== null &&
    selectedAlternativeId.trim().length > 0 &&
    alternatives.some((alt) => alt.id.trim() === selectedAlternativeId.trim()) &&
    selectionRationale.trim().length > 0;
  if (!validAlternatives) {
    blockers.push("alternatives_incomplete");
  }

  const strategy = plan?.strategy ?? detail.strategy ?? null;
  const strategyRationale = plan?.strategyRationale ?? detail.strategyRationale ?? "";
  const strategyMetadata = plan?.strategyMetadata ?? detail.strategyMetadata ?? null;
  if (!isPlanStrategyValid({ strategy, strategyRationale, strategyMetadata })) {
    blockers.push("strategy_incomplete");
  }

  const granularity: WorkflowBoardTaskGranularity | null =
    plan?.granularity ?? detail.granularity ?? null;
  if (granularity === null) {
    blockers.push("granularity_missing");
  }

  const tasks: ReadonlyArray<WorkflowBoardTaskContract> = plan?.tasks ?? detail.tasks ?? [];
  if (tasks.length === 0 || !isValidTaskContractsArray(tasks)) {
    blockers.push("tasks_incomplete");
  }

  const checks: ReadonlyArray<WorkflowBoardPlannedCheck> = plan?.checks ?? detail.checks ?? [];
  if (checks.some((check) => check.id.trim() === "" || !isSafeWorkflowBoardArgv(check.argv))) {
    blockers.push("checks_invalid");
  }

  const acMappings: ReadonlyArray<WorkflowBoardAcEvidenceMapping> = normalizeAcEvidenceMappings(
    plan?.acMappings ?? detail.acMappings ?? [],
  );
  const mappedIds = new Set(acMappings.map((entry) => entry.criterionId.trim()));
  const allCriteriaMapped =
    isValidAcEvidenceMappingsArray(acMappings) &&
    detail.spec.acceptanceCriteria.every((criterion) => mappedIds.has(criterion.id.trim()));
  if (!allCriteriaMapped) {
    blockers.push("ac_mappings_incomplete");
  }

  return blockers;
}

export function canApproveWorkflowBoardPlan(input: {
  readonly detail: WorkflowCardDetail | null;
  readonly canOperate: boolean;
}): boolean {
  if (!input.canOperate || input.detail === null || input.detail.stage !== "plan") {
    return false;
  }
  return collectPlanApprovalBlockers(input.detail).length === 0;
}

export function collectDownstreamInvalidationStates(
  card: Pick<
    WorkflowCardDetail,
    | "specApproval"
    | "specApprovalStatus"
    | "planApproval"
    | "planApprovalStatus"
    | "challengeReceipt"
    | "blockerSummary"
  >,
): {
  readonly specStale: boolean;
  readonly planStale: boolean;
  readonly challengeStale: boolean;
  readonly externalEditNotice: boolean;
} {
  const blocker = card.blockerSummary?.toLowerCase() ?? "";
  return {
    specStale: card.specApproval?.status === "stale" || card.specApprovalStatus === "stale",
    planStale: card.planApproval?.status === "stale" || card.planApprovalStatus === "stale",
    challengeStale: card.challengeReceipt?.status === "stale",
    externalEditNotice:
      blocker.includes("external") ||
      blocker.includes("digest") ||
      blocker.includes("edited on disk"),
  };
}

export function validReturnTargetStages(
  stage: WorkflowBoardStage,
): readonly WorkflowBoardReturnTargetStage[] {
  switch (stage) {
    case "spec":
      return [];
    case "plan":
      return ["spec"];
    case "execute":
      return ["spec", "plan"];
    case "ship":
      return ["spec", "plan", "execute"];
  }
}

export function isCheckReceiptStale(
  receipt: Pick<WorkflowBoardPlannedCheckReceipt, "status" | "productSnapshotDigest">,
  currentSnapshotDigest: string | null,
): boolean {
  if (receipt.status === "stale" || receipt.status === "historical") return true;
  return (
    currentSnapshotDigest !== null &&
    receipt.productSnapshotDigest !== null &&
    receipt.productSnapshotDigest !== currentSnapshotDigest
  );
}

export function isReviewReportStale(
  report: Pick<WorkflowBoardReviewReport, "status" | "verdict" | "productSnapshotDigest">,
  currentSnapshotDigest: string | null,
): boolean {
  if (report.status === "stale" || report.status === "historical" || report.verdict === "stale") {
    return true;
  }
  return (
    currentSnapshotDigest !== null &&
    report.productSnapshotDigest !== null &&
    report.productSnapshotDigest !== currentSnapshotDigest
  );
}

export function isAcObservationStale(
  observation: Pick<WorkflowBoardAcObservationRecord, "status" | "productSnapshotDigest">,
  currentSnapshotDigest: string | null,
): boolean {
  if (observation.status === "stale" || observation.status === "historical") return true;
  return (
    currentSnapshotDigest !== null &&
    observation.productSnapshotDigest !== null &&
    observation.productSnapshotDigest !== currentSnapshotDigest
  );
}

export interface WorkflowBoardFixNavigation {
  readonly subTab: "spec" | "plan" | "execute" | "ship";
  readonly executeSection?: "recovery" | "policies" | "tasks" | "checks" | "review";
  readonly shipSection?: "acceptance";
  readonly targetId: string | null;
}

export function resolveFixPointerNavigation(
  pointer: WorkflowBoardFixPointer,
): WorkflowBoardFixNavigation {
  switch (pointer.stage) {
    case "spec":
      return { subTab: "spec", targetId: pointer.targetId };
    case "plan":
      return { subTab: "plan", targetId: pointer.targetId };
    case "execute": {
      const executeSection =
        pointer.tab === "recovery" ||
        pointer.tab === "policies" ||
        pointer.tab === "tasks" ||
        pointer.tab === "checks" ||
        pointer.tab === "review"
          ? pointer.tab
          : "tasks";
      return { subTab: "execute", executeSection, targetId: pointer.targetId };
    }
    case "ship":
      return {
        subTab: "ship",
        ...(pointer.tab === "acceptance" ? { shipSection: "acceptance" as const } : {}),
        targetId: pointer.targetId,
      };
  }
}

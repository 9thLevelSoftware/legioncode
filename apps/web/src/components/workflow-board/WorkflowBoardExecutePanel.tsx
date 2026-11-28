import { useAtomValue } from "@effect/atom-react";
import {
  workflowBoardCheckStatusLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardLatestStageRun,
  workflowBoardRecoverySummary,
} from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  WorkflowBoardArtifactKey,
  type EnvironmentId,
  type ModelSelection,
  type ProjectId,
  type ProviderInteractionMode,
  type RuntimeMode,
  type ServerProvider,
  type ServerSettings,
  type ThreadId,
  type WorkflowBoardAmendmentTargetStage,
  type WorkflowBoardCardSummary,
  type WorkflowBoardGetCardDiffResult,
  type WorkflowBoardPlannedCheckReceipt,
  type WorkflowBoardTaskContract,
  type WorkflowBoardTaskExecutionState,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import {
  AlertTriangleIcon,
  ExternalLinkIcon,
  FileEditIcon,
  PauseIcon,
  PlayIcon,
  RefreshCwIcon,
  RotateCcwIcon,
  TerminalIcon,
  WrenchIcon,
} from "lucide-react";
import { useId, useState } from "react";

import { cn } from "../../lib/utils";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Checkbox } from "../ui/checkbox";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  buildAmendedTaskContractsWithUnexpectedFiles,
  canStartExecuteTask,
  findInProgressTask,
  formatArgvTokens,
  isCheckReceiptStale,
  isTaskDagReady,
  needsCardReconciliation,
  normalizeWorkspaceStrategy,
  resolveCrossColumnTransition,
  taskDependencyIds,
  unacknowledgedBlockingCliPolicies,
  type WorkflowBoardCrossColumnTransition,
} from "./workflowBoard.logic";
import { WorkflowBoardCardDiffViewer } from "./WorkflowBoardCardDiffViewer";
import { WorkflowBoardLaunchControls } from "./WorkflowBoardLaunchControls";
import { WorkflowBoardReviewPanel } from "./WorkflowBoardReviewPanel";

function toSummary(card: WorkflowCardDetail): WorkflowBoardCardSummary {
  return {
    cardId: card.cardId,
    projectId: card.projectId,
    title: card.title,
    stage: card.stage,
    status: card.status,
    revision: card.revision,
    currentTask: card.currentTask,
    blockerSummary: card.blockerSummary,
    nextAction: card.nextAction,
    specApprovedRevision: card.specApprovedRevision,
    specApprovalStatus: card.specApprovalStatus,
    activeThreadId: card.activeThreadId,
    createdAt: card.createdAt,
    updatedAt: card.updatedAt,
    planApprovedRevision: card.planApprovedRevision,
    planApprovalStatus: card.planApprovalStatus,
    incompleteJournalCount: card.incompleteJournalCount,
    hasUnreconciledEdits: card.hasUnreconciledEdits,
    deliveryStatus: card.deliveryStatus,
  };
}

interface AmendmentFormDraft {
  readonly open: boolean;
  readonly targetStage: WorkflowBoardAmendmentTargetStage;
  readonly reason: string;
  readonly summary: string;
  readonly sourceTaskId: string | null;
  readonly unexpectedFiles: ReadonlyArray<WorkflowBoardArtifactKey>;
  readonly includeUnexpectedInTask: boolean;
}

const DEFAULT_AMENDMENT_DRAFT: AmendmentFormDraft = {
  open: false,
  targetStage: "plan",
  reason: "",
  summary: "",
  sourceTaskId: null,
  unexpectedFiles: [],
  includeUnexpectedInTask: false,
};

export function WorkflowBoardExecutePanel(props: {
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly card: WorkflowCardDetail;
  readonly cardDiff: WorkflowBoardGetCardDiffResult | null;
  readonly cardDiffLoading: boolean;
  readonly cardDiffError: string | null;
  readonly onRefreshDiff: () => void;
  readonly onRefreshCard: () => void;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly existingWorktreePaths: ReadonlyArray<string>;
  readonly modelSelection: ModelSelection | null;
  readonly onModelSelectionChange: (next: ModelSelection) => void;
  readonly runtimeMode: RuntimeMode;
  readonly onRuntimeModeChange: (next: RuntimeMode) => void;
  readonly interactionMode: ProviderInteractionMode;
  readonly onInteractionModeChange: (next: ProviderInteractionMode) => void;
  readonly workspaceStrategy: WorkflowCardDetail["workspaceStrategy"];
  readonly onWorkspaceStrategyChange: (next: WorkflowCardDetail["workspaceStrategy"]) => void;
  readonly workspaceStrategyValid: boolean;
  readonly focusedSection?: "recovery" | "policies" | "tasks" | "checks" | "review" | null;
  readonly highlightedTargetId?: string | null;
  readonly onSelectThread: (stage: "execute" | "review", threadId: ThreadId) => void;
  readonly onStageStarted?: (threadId: ThreadId) => void;
  readonly onRequestTransition?: (
    card: WorkflowBoardCardSummary,
    transition: WorkflowBoardCrossColumnTransition,
  ) => void;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const amendmentErrorId = useId();
  const includeUnexpectedCheckboxId = useId();
  const acknowledgePoliciesCheckboxId = useId();
  const { card } = props;

  const canStartStage = useAtomValue(workflowBoard.startStage.permissionAtom(props.environmentId));
  const canPauseCard = useAtomValue(workflowBoard.pauseCard.permissionAtom(props.environmentId));
  const canResumeCard = useAtomValue(workflowBoard.resumeCard.permissionAtom(props.environmentId));
  const canRetryStep = useAtomValue(workflowBoard.retryStep.permissionAtom(props.environmentId));
  const canAmend = useAtomValue(workflowBoard.proposeAmendment.permissionAtom(props.environmentId));
  const canReturn = useAtomValue(
    workflowBoard.returnForRevision.permissionAtom(props.environmentId),
  );

  const startStage = useAtomCommand(workflowBoard.startStage, {
    label: "workflow board start stage",
    reportFailure: false,
  });
  const pauseCard = useAtomCommand(workflowBoard.pauseCard, {
    label: "workflow board pause card",
    reportFailure: false,
  });
  const resumeCard = useAtomCommand(workflowBoard.resumeCard, {
    label: "workflow board resume card",
    reportFailure: false,
  });
  const retryStep = useAtomCommand(workflowBoard.retryStep, {
    label: "workflow board retry step",
    reportFailure: false,
  });
  const proposeAmendment = useAtomCommand(workflowBoard.proposeAmendment, {
    label: "workflow board propose amendment",
    reportFailure: false,
  });

  const [pauseReason, setPauseReason] = useState("");
  const [amendmentDraft, setAmendmentDraft] = useState<AmendmentFormDraft>(DEFAULT_AMENDMENT_DRAFT);
  const [amendmentValidationError, setAmendmentValidationError] = useState<string | null>(null);
  const [acknowledgedPolicySignature, setAcknowledgedPolicySignature] = useState<string | null>(
    null,
  );
  const [busyAction, setBusyAction] = useState<string | null>(null);

  const unsupportedPolicies = card.cliImport?.unsupportedPolicies ?? [];
  const unacknowledgedBlocking = unacknowledgedBlockingCliPolicies(card);
  const unacknowledgedPolicySignature =
    unacknowledgedBlocking.length > 0
      ? `${card.cardId}:${card.cliImport?.importedAt ?? ""}:${card.cliImport?.lastSyncedAt ?? ""}:${unacknowledgedBlocking
          .map((policy) => policy.key)
          .sort()
          .join("|")}`
      : "";
  if (
    acknowledgedPolicySignature !== null &&
    acknowledgedPolicySignature !== unacknowledgedPolicySignature
  ) {
    setAcknowledgedPolicySignature(null);
  }
  const policiesAcknowledged =
    unacknowledgedPolicySignature.length > 0 &&
    acknowledgedPolicySignature === unacknowledgedPolicySignature;
  const missingLogNotes = card.cliImport?.missingLogNotes ?? [];

  const tasks: ReadonlyArray<WorkflowBoardTaskContract> = card.plan?.tasks ?? card.tasks ?? [];
  const executions: ReadonlyArray<WorkflowBoardTaskExecutionState> = card.taskExecutions ?? [];
  const executionByTask = new Map(executions.map((exec) => [exec.taskId, exec]));
  const activeTaskId = findInProgressTask(tasks, executions);

  const violations = (card.fileContractReports ?? []).filter(
    (report) => report.status === "violation_pending",
  );
  const violationTaskIds = new Set(violations.map((report) => report.taskId));

  const recoverySummary = workflowBoardRecoverySummary(card);
  const needsReconciliation = needsCardReconciliation(card);
  const latestExecuteRun = workflowBoardLatestStageRun(card, "execute");
  const activeWorktreePath =
    props.cardDiff?.worktreePath ??
    executions.find((exec) => exec.worktreePath !== null)?.worktreePath ??
    latestExecuteRun?.worktreePath ??
    null;
  const activeBranch =
    executions.find((exec) => exec.branch !== null)?.branch ?? latestExecuteRun?.branch ?? null;
  const currentSnapshotDigest =
    props.cardDiff?.productSnapshotDigest ?? card.shipReadiness?.productSnapshotDigest ?? null;

  const checkReceipts: ReadonlyArray<WorkflowBoardPlannedCheckReceipt> =
    card.checkReceipts && card.checkReceipts.length > 0
      ? card.checkReceipts
      : (card.plan?.checks ?? card.checks ?? []).map((planned) => ({
          checkId: planned.id,
          criterionId: planned.criterionId ?? null,
          ownerTaskId: null,
          argv: planned.argv,
          description: planned.description ?? null,
          status: "pending" as const,
          exitCode: null,
          logRef: null,
          productSnapshotDigest: null,
          executedAt: null,
          updatedAt: card.updatedAt,
        }));

  const workflowsEnabled = props.settings.workflowsEnabled === true;
  const returnToPlanTransition = resolveCrossColumnTransition(toSummary(card), "plan");
  const isCompleted = card.status === "completed";

  const handlePause = async () => {
    if (!canPauseCard || card.status === "paused" || isCompleted || busyAction !== null) return;
    setBusyAction("pause");
    props.onActionError(null);
    props.onActionNotice(null);

    const trimmedReason = pauseReason.trim();
    const result = await pauseCard({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        ...(trimmedReason.length > 0 ? { reason: trimmedReason } : {}),
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not pause card."),
      );
      return;
    }
    setPauseReason("");
    props.onActionNotice(gt("Card paused."));
  };

  const handleResume = async (reconcileInterrupted: boolean) => {
    if (!canResumeCard || card.status !== "paused" || busyAction !== null) return;
    setBusyAction(reconcileInterrupted ? "reconcile-resume" : "resume");
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await resumeCard({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        ...(reconcileInterrupted ? { reconcileInterrupted: true } : {}),
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not resume card."),
      );
      return;
    }
    props.onRefreshDiff();
    props.onActionNotice(
      reconcileInterrupted
        ? gt("Reconciled {count} pending item(s) and resumed card.", {
            count: result.value.reconciledCount,
          })
        : gt("Card resumed."),
    );
  };

  const handleRetryStage = async () => {
    if (!canRetryStep || card.stage !== "execute" || isCompleted || busyAction !== null) return;
    setBusyAction("retry-stage");
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await retryStep({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stepKind: "stage",
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not reset Execute stage."),
      );
      return;
    }
    props.onActionNotice(gt("Execute stage reset for retry."));
  };

  const handleStartTask = async (taskId: string) => {
    if (props.modelSelection === null || busyAction !== null) return;
    setBusyAction(`start-task:${taskId}`);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await startStage({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stage: "execute",
        taskId,
        ...(unacknowledgedBlocking.length > 0 && policiesAcknowledged
          ? {
              acknowledgeUnsupportedPolicies: true,
              acknowledgedPolicies: unacknowledgedBlocking.map((policy) => policy.key),
            }
          : {}),
        modelSelection: props.modelSelection,
        runtimeMode: props.runtimeMode,
        interactionMode: props.interactionMode,
        workspaceStrategy: normalizeWorkspaceStrategy(props.workspaceStrategy),
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not start task {taskId}.", { taskId }),
      );
      return;
    }
    props.onSelectThread("execute", result.value.threadId);
    props.onStageStarted?.(result.value.threadId);
    props.onRefreshDiff();
    props.onActionNotice(
      result.value.resumed
        ? gt("Reconnected to task {taskId} thread.", { taskId })
        : gt("Started task {taskId}.", { taskId }),
    );
  };

  const handleRetryTask = async (taskId: string) => {
    if (!canRetryStep || busyAction !== null) return;
    setBusyAction(`retry-task:${taskId}`);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await retryStep({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stepKind: "task",
        targetId: taskId,
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not retry task {taskId}.", { taskId }),
      );
      return;
    }
    props.onActionNotice(gt("Task {taskId} reset to todo.", { taskId }));
  };

  const handleRunCheck = async (checkId: string) => {
    if (!canRetryStep || card.status === "paused" || isCompleted || busyAction !== null) return;
    setBusyAction(`check:${checkId}`);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await retryStep({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stepKind: "check",
        targetId: checkId,
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not run check {checkId}.", { checkId }),
      );
      return;
    }
    props.onRefreshDiff();
    props.onActionNotice(gt("Executed planned check {checkId}.", { checkId }));
  };

  const openAmendmentForViolation = (
    taskId: string,
    unexpectedFiles: ReadonlyArray<WorkflowBoardArtifactKey>,
  ) => {
    setAmendmentValidationError(null);
    setAmendmentDraft({
      open: true,
      targetStage: "plan",
      reason: gt("Amend file contract after unexpected edits on {taskId}", { taskId }),
      summary: "",
      sourceTaskId: taskId,
      unexpectedFiles,
      includeUnexpectedInTask: unexpectedFiles.length > 0,
    });
  };

  const handleSubmitAmendment = async () => {
    if (!canAmend || busyAction !== null) return;
    const trimmedReason = amendmentDraft.reason.trim();
    if (trimmedReason.length === 0) {
      setAmendmentValidationError(gt("Enter a reason for the plan amendment."));
      return;
    }

    let amendedTasks: ReadonlyArray<WorkflowBoardTaskContract> | undefined;
    if (
      amendmentDraft.includeUnexpectedInTask &&
      amendmentDraft.sourceTaskId !== null &&
      amendmentDraft.unexpectedFiles.length > 0 &&
      tasks.length > 0
    ) {
      const amended = buildAmendedTaskContractsWithUnexpectedFiles({
        tasks,
        sourceTaskId: amendmentDraft.sourceTaskId,
        unexpectedFiles: amendmentDraft.unexpectedFiles,
      });
      if (!amended.ok) {
        setAmendmentValidationError(
          amended.reason === "unsafe_non_git_path"
            ? gt(
                "Git metadata paths (.git) cannot be added to a task's allowed files. Uncheck including unexpected files or adjust the task in Plan.",
              )
            : gt(
                "Including those files would conflict with another task's file contract. Adjust files in Plan after proposing the amendment.",
              ),
        );
        return;
      }
      amendedTasks = amended.tasks;
    }

    setAmendmentValidationError(null);
    setBusyAction("amend");
    props.onActionError(null);
    props.onActionNotice(null);

    const trimmedSummary = amendmentDraft.summary.trim();
    const result = await proposeAmendment({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        targetStage: amendmentDraft.targetStage,
        reason: trimmedReason,
        ...(trimmedSummary.length > 0 ? { summary: trimmedSummary } : {}),
        ...(amendmentDraft.sourceTaskId !== null
          ? { sourceTaskId: amendmentDraft.sourceTaskId }
          : {}),
        ...(amendmentDraft.unexpectedFiles.length > 0
          ? { unexpectedFiles: amendmentDraft.unexpectedFiles }
          : {}),
        ...(amendedTasks !== undefined ? { tasks: amendedTasks } : {}),
      },
    });
    setBusyAction(null);
    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not propose plan amendment."),
      );
      return;
    }
    setAmendmentDraft(DEFAULT_AMENDMENT_DRAFT);
    props.onActionNotice(
      gt("Amendment proposed. Review and re-approve Plan before resuming execution."),
    );
  };

  return (
    <div className="flex flex-col gap-5">
      <section
        aria-labelledby="execute-lifecycle-heading"
        className="flex flex-col gap-3 rounded-lg border bg-card p-4"
      >
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <h3 id="execute-lifecycle-heading" className="text-sm font-semibold text-foreground">
              <T>Execution lifecycle</T>
            </h3>
            <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              {activeWorktreePath !== null ? (
                <span className="font-mono">
                  <T>
                    worktree <Var>{activeWorktreePath}</Var>
                  </T>
                </span>
              ) : null}
              {activeBranch !== null ? (
                <Badge variant="outline" size="sm">
                  {activeBranch}
                </Badge>
              ) : null}
              {currentSnapshotDigest !== null ? (
                <span className="font-mono">
                  <T>
                    snapshot <Var>{currentSnapshotDigest}</Var>
                  </T>
                </span>
              ) : null}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            {card.status === "paused" ? (
              <>
                {needsReconciliation ? (
                  <Button
                    type="button"
                    size="xs"
                    disabled={!canResumeCard || busyAction !== null}
                    onClick={() => void handleResume(true)}
                  >
                    <RefreshCwIcon />
                    {busyAction === "reconcile-resume" ? (
                      <T>Reconciling…</T>
                    ) : (
                      <T>Reconcile & resume</T>
                    )}
                  </Button>
                ) : null}
                <Button
                  type="button"
                  size="xs"
                  variant={needsReconciliation ? "outline" : "default"}
                  disabled={!canResumeCard || busyAction !== null}
                  onClick={() => void handleResume(false)}
                >
                  <PlayIcon />
                  {busyAction === "resume" ? <T>Resuming…</T> : <T>Resume card</T>}
                </Button>
              </>
            ) : !isCompleted ? (
              <>
                <Button
                  type="button"
                  size="xs"
                  variant="outline"
                  disabled={!canPauseCard || busyAction !== null}
                  onClick={() => void handlePause()}
                >
                  <PauseIcon />
                  {busyAction === "pause" ? <T>Pausing…</T> : <T>Pause card</T>}
                </Button>
                <Button
                  type="button"
                  size="xs"
                  variant="outline"
                  disabled={!canRetryStep || card.stage !== "execute" || busyAction !== null}
                  onClick={() => void handleRetryStage()}
                >
                  <RotateCcwIcon />
                  <T>Retry stage</T>
                </Button>
              </>
            ) : null}

            {!isCompleted ? (
              <Button
                type="button"
                size="xs"
                variant="outline"
                aria-expanded={amendmentDraft.open}
                disabled={!canAmend || busyAction !== null}
                onClick={() => {
                  setAmendmentValidationError(null);
                  setAmendmentDraft((current) => ({
                    ...current,
                    open: !current.open,
                  }));
                }}
              >
                <FileEditIcon />
                <T>Propose plan amendment</T>
              </Button>
            ) : null}
          </div>
        </div>

        {card.status !== "paused" && !isCompleted ? (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              aria-label={gt("Pause reason")}
              placeholder={gt("Optional pause reason...")}
              value={pauseReason}
              disabled={!canPauseCard || busyAction !== null}
              onChange={(event) => setPauseReason(event.target.value)}
              className="max-w-sm"
            />
          </div>
        ) : null}

        {amendmentDraft.open ? (
          <div className="flex flex-col gap-3 rounded-md border bg-background p-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-semibold text-foreground">
                <T>Propose plan amendment</T>
              </span>
              <Select
                value={amendmentDraft.targetStage}
                disabled={busyAction !== null}
                onValueChange={(value) => {
                  if (value === "spec" || value === "plan") {
                    setAmendmentDraft((current) => ({ ...current, targetStage: value }));
                  }
                }}
              >
                <SelectTrigger size="sm" aria-label={gt("Amendment target stage")}>
                  <SelectValue>
                    {amendmentDraft.targetStage === "spec" ? gt("Spec") : gt("Plan")}
                  </SelectValue>
                </SelectTrigger>
                <SelectPopup>
                  <SelectItem value="plan">{gt("Plan")}</SelectItem>
                  <SelectItem value="spec">{gt("Spec")}</SelectItem>
                </SelectPopup>
              </Select>
            </div>

            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="workflow-amendment-reason"
                className="text-xs font-medium text-foreground"
              >
                <T>Reason</T>
              </label>
              <Textarea
                id="workflow-amendment-reason"
                rows={2}
                required
                aria-invalid={amendmentValidationError !== null || undefined}
                aria-describedby={amendmentValidationError !== null ? amendmentErrorId : undefined}
                placeholder={gt("Describe why the plan or file contract must change...")}
                value={amendmentDraft.reason}
                disabled={busyAction !== null}
                onChange={(event) => {
                  setAmendmentValidationError(null);
                  setAmendmentDraft((current) => ({ ...current, reason: event.target.value }));
                }}
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="workflow-amendment-summary"
                className="text-xs font-medium text-foreground"
              >
                <T>Summary (optional)</T>
              </label>
              <Input
                id="workflow-amendment-summary"
                placeholder={gt("Optional summary of scope changes...")}
                value={amendmentDraft.summary}
                disabled={busyAction !== null}
                onChange={(event) =>
                  setAmendmentDraft((current) => ({ ...current, summary: event.target.value }))
                }
              />
            </div>

            {amendmentDraft.unexpectedFiles.length > 0 && amendmentDraft.sourceTaskId !== null ? (
              <label
                htmlFor={includeUnexpectedCheckboxId}
                className="flex items-center gap-2 text-xs text-foreground"
              >
                <Checkbox
                  id={includeUnexpectedCheckboxId}
                  checked={amendmentDraft.includeUnexpectedInTask}
                  disabled={busyAction !== null}
                  onCheckedChange={(checked) => {
                    setAmendmentValidationError(null);
                    setAmendmentDraft((current) => ({
                      ...current,
                      includeUnexpectedInTask: checked === true,
                    }));
                  }}
                />
                <T>
                  Include unexpected file(s) (<Var>{amendmentDraft.unexpectedFiles.join(", ")}</Var>
                  ) in <Var>{amendmentDraft.sourceTaskId}</Var> allowed files
                </T>
              </label>
            ) : null}

            {amendmentValidationError !== null ? (
              <p id={amendmentErrorId} role="alert" className="text-xs text-destructive-foreground">
                {amendmentValidationError}
              </p>
            ) : null}

            <div className="flex items-center justify-end gap-2">
              <Button
                type="button"
                size="xs"
                variant="ghost"
                disabled={busyAction !== null}
                onClick={() => {
                  setAmendmentValidationError(null);
                  setAmendmentDraft(DEFAULT_AMENDMENT_DRAFT);
                }}
              >
                <T>Cancel</T>
              </Button>
              <Button
                type="button"
                size="xs"
                disabled={!canAmend || busyAction !== null}
                onClick={() => void handleSubmitAmendment()}
              >
                {busyAction === "amend" ? <T>Submitting…</T> : <T>Submit amendment</T>}
              </Button>
            </div>
          </div>
        ) : null}

        {(card.amendments ?? []).length > 0 ? (
          <div className="flex flex-col gap-1.5 border-t pt-2">
            <span className="text-xs font-medium text-muted-foreground">
              <T>Amendment history</T>
            </span>
            <ul className="flex flex-col gap-1">
              {(card.amendments ?? []).map((amendment) => (
                <li
                  key={amendment.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded bg-muted/40 px-2 py-1 text-xs"
                >
                  <span>
                    <span className="font-mono">{amendment.id}</span> · {amendment.reason}
                  </span>
                  <Badge variant="outline" size="sm">
                    {amendment.status ?? "proposed"}
                  </Badge>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
      </section>

      {needsReconciliation ? (
        <section
          data-execute-section="recovery"
          aria-labelledby="execute-recovery-heading"
          tabIndex={-1}
          className={cn(
            "flex flex-col gap-3 rounded-lg border p-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
            props.focusedSection === "recovery"
              ? "border-primary ring-1 ring-primary"
              : "border-warning/60 bg-warning/5",
          )}
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <AlertTriangleIcon className="size-4 text-warning-foreground" />
              <h3 id="execute-recovery-heading" className="text-sm font-semibold text-foreground">
                <T>Reconciliation required</T>
              </h3>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                size="xs"
                variant="outline"
                onClick={() => {
                  props.onRefreshCard();
                  props.onRefreshDiff();
                }}
              >
                <RefreshCwIcon />
                <T>Refresh state</T>
              </Button>
              {card.status === "paused" ? (
                <Button
                  type="button"
                  size="xs"
                  disabled={!canResumeCard || busyAction !== null}
                  onClick={() => void handleResume(true)}
                >
                  <T>Reconcile & resume</T>
                </Button>
              ) : (
                <Button
                  type="button"
                  size="xs"
                  disabled={!canPauseCard || busyAction !== null}
                  onClick={() => void handlePause()}
                >
                  <T>Pause to reconcile</T>
                </Button>
              )}
            </div>
          </div>

          <p className="text-xs text-muted-foreground">
            <T>
              External file edits under .legion/workflows/<Var>{card.cardId}</Var>/ or interrupted
              write-journal actions must be reconciled before starting new tasks, checks, reviews,
              or delivery.
            </T>
          </p>

          {recoverySummary.pendingJournalEntries.length > 0 ? (
            <ul className="flex flex-col gap-1.5 text-xs">
              {recoverySummary.pendingJournalEntries.map((entry) => (
                <li
                  key={entry.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded border bg-background px-2.5 py-1.5"
                >
                  <span className="font-mono">
                    {entry.operation}: {entry.targetPath}
                  </span>
                  <Badge variant="warning" size="sm">
                    {entry.status}
                  </Badge>
                </li>
              ))}
            </ul>
          ) : null}

          {recoverySummary.detectedExternalEdits.length > 0 ? (
            <ul className="flex flex-col gap-1.5 text-xs">
              {recoverySummary.detectedExternalEdits.map((edit) => (
                <li
                  key={edit.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded border bg-background px-2.5 py-1.5"
                >
                  <div>
                    <span className="font-mono font-medium">{edit.relativePath}</span>
                    <p className="text-muted-foreground">{edit.summary}</p>
                  </div>
                  <Badge variant="warning" size="sm">
                    {edit.affectedGates.join(", ")}
                  </Badge>
                </li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}

      {unsupportedPolicies.length > 0 ? (
        <section
          data-execute-section="policies"
          aria-labelledby="execute-policies-heading"
          tabIndex={-1}
          className={cn(
            "flex flex-col gap-3 rounded-lg border p-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
            props.focusedSection === "policies" ? "border-primary ring-1 ring-primary" : undefined,
          )}
        >
          <Alert variant={unacknowledgedBlocking.length > 0 ? "warning" : "info"} role="note">
            <AlertTriangleIcon />
            <AlertTitle id="execute-policies-heading">
              <T>
                Unsupported .legion-cli policies (<Var>{unsupportedPolicies.length}</Var>)
              </T>
            </AlertTitle>
            <AlertDescription>
              <ul className="mt-1 flex flex-col gap-2">
                {unsupportedPolicies.map((policy) => {
                  const isHighlighted = props.highlightedTargetId === policy.key;
                  return (
                    <li
                      key={policy.key}
                      data-cli-policy-key={policy.key}
                      tabIndex={-1}
                      className={cn(
                        "flex flex-col gap-1 rounded border bg-background/80 p-2 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                        isHighlighted ? "border-primary ring-1 ring-primary" : undefined,
                      )}
                    >
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="font-mono font-semibold text-foreground">
                            {policy.key}
                          </span>
                          <Badge variant="outline" size="sm">
                            {policy.category}
                          </Badge>
                          <Badge
                            variant={policy.status === "acknowledged" ? "success" : "warning"}
                            size="sm"
                          >
                            {workflowBoardCliImportPolicyStatusLabel(policy.status, messages)}
                          </Badge>
                          {policy.blocksExecution ? (
                            <Badge variant="error" size="sm">
                              <T>Blocks Execute</T>
                            </Badge>
                          ) : null}
                        </div>
                        <span className="font-mono text-2xs text-muted-foreground">
                          {policy.sourcePath}
                        </span>
                      </div>
                      <p className="text-foreground">{policy.summary}</p>
                      {policy.configuredValue !== null ? (
                        <p className="font-mono text-2xs text-muted-foreground">
                          <T>
                            Configured: <Var>{policy.configuredValue}</Var>
                          </T>
                        </p>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
              {unacknowledgedBlocking.length > 0 && !isCompleted ? (
                <label
                  htmlFor={acknowledgePoliciesCheckboxId}
                  className="mt-3 flex cursor-pointer items-center gap-2 text-xs text-foreground"
                >
                  <Checkbox
                    id={acknowledgePoliciesCheckboxId}
                    checked={policiesAcknowledged}
                    onCheckedChange={(checked) =>
                      setAcknowledgedPolicySignature(checked ? unacknowledgedPolicySignature : null)
                    }
                    disabled={busyAction !== null}
                  />
                  <span>
                    <T>
                      I acknowledge these unsupported .legion-cli policies and want to run Execute
                      in LegionCode
                    </T>
                  </span>
                </label>
              ) : null}
            </AlertDescription>
          </Alert>
        </section>
      ) : null}

      {violations.map((report) => (
        <section
          key={report.id ?? `${report.taskId}-${report.updatedAt}`}
          data-contract-violation-task={report.taskId}
          aria-labelledby={`execute-violation-heading-${report.taskId}`}
          className="flex flex-col gap-3 rounded-lg border border-warning bg-warning/5 p-4"
        >
          <div className="flex flex-wrap items-start justify-between gap-2">
            <div className="flex flex-col gap-1">
              <div className="flex items-center gap-2">
                <AlertTriangleIcon className="size-4 text-warning-foreground" />
                <h3
                  id={`execute-violation-heading-${report.taskId}`}
                  className="text-sm font-semibold text-foreground"
                >
                  <T>
                    Unexpected edits preserved for review (<Var>{report.taskId}</Var>)
                  </T>
                </h3>
              </div>
              <p className="text-xs text-muted-foreground">
                {report.preservedInWorktree ? (
                  <T>
                    Files outside the approved task contract were preserved intact in the worktree
                    for inspection rather than discarded. Propose a plan amendment or return to Plan
                    to update file ownership.
                  </T>
                ) : (
                  (report.blockingReason ?? gt("Task edits violated the approved file contract."))
                )}
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              {returnToPlanTransition !== null && props.onRequestTransition !== undefined ? (
                <Button
                  type="button"
                  size="xs"
                  variant="ghost"
                  disabled={!canReturn}
                  onClick={() =>
                    props.onRequestTransition?.(toSummary(card), returnToPlanTransition)
                  }
                >
                  <T>Return to Plan</T>
                </Button>
              ) : null}
              <Button
                type="button"
                size="xs"
                disabled={!canAmend}
                onClick={() => openAmendmentForViolation(report.taskId, report.unexpectedFiles)}
              >
                <FileEditIcon />
                <T>Propose plan amendment</T>
              </Button>
            </div>
          </div>

          <div className="grid gap-2 text-xs sm:grid-cols-3">
            <div className="rounded border bg-background p-2">
              <span className="font-medium text-foreground">
                <T>Allowed</T>
              </span>
              <p className="mt-1 font-mono text-muted-foreground">
                {report.allowedFiles.length > 0 ? report.allowedFiles.join(", ") : gt("None")}
              </p>
            </div>
            <div className="rounded border bg-background p-2">
              <span className="font-medium text-warning-foreground">
                <T>Unexpected</T>
              </span>
              <p className="mt-1 font-mono text-foreground">
                {report.unexpectedFiles.length > 0 ? report.unexpectedFiles.join(", ") : gt("None")}
              </p>
            </div>
            <div className="rounded border bg-background p-2">
              <span className="font-medium text-destructive-foreground">
                <T>Forbidden</T>
              </span>
              <p className="mt-1 font-mono text-foreground">
                {report.forbiddenFiles.length > 0 ? report.forbiddenFiles.join(", ") : gt("None")}
              </p>
            </div>
          </div>

          <WorkflowBoardCardDiffViewer
            diff={props.cardDiff}
            loading={props.cardDiffLoading}
            error={props.cardDiffError}
            filterPaths={[
              ...report.unexpectedFiles,
              ...report.forbiddenFiles,
              ...report.changedFiles,
            ]}
            onRefresh={props.onRefreshDiff}
          />
        </section>
      ))}

      <section
        data-execute-section="tasks"
        aria-labelledby="execute-tasks-heading"
        tabIndex={-1}
        className={cn(
          "flex flex-col gap-3 rounded-lg border bg-card p-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
          props.focusedSection === "tasks" ? "border-primary ring-1 ring-primary" : undefined,
        )}
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <WrenchIcon className="size-4 text-muted-foreground" />
            <h3 id="execute-tasks-heading" className="text-sm font-semibold text-foreground">
              <T>Task queue & DAG</T>
            </h3>
          </div>
          {activeTaskId !== null ? (
            <Badge variant="warning" size="sm">
              <T>
                Active mutating task: <Var>{activeTaskId}</Var>
              </T>
            </Badge>
          ) : null}
        </div>

        <WorkflowBoardLaunchControls
          environmentId={props.environmentId}
          projectId={props.projectId}
          providers={props.providers}
          settings={props.settings}
          existingWorktreePaths={props.existingWorktreePaths}
          modelSelection={props.modelSelection}
          onModelSelectionChange={props.onModelSelectionChange}
          runtimeMode={props.runtimeMode}
          onRuntimeModeChange={props.onRuntimeModeChange}
          interactionMode={props.interactionMode}
          onInteractionModeChange={props.onInteractionModeChange}
          workspaceStrategy={props.workspaceStrategy}
          onWorkspaceStrategyChange={props.onWorkspaceStrategyChange}
          disabled={isCompleted}
        />

        {tasks.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            <T>No tasks defined in the approved plan.</T>
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {tasks.map((task) => {
              const exec = executionByTask.get(task.taskId) ?? null;
              const status = exec?.status ?? task.status ?? "todo";
              const deps = taskDependencyIds(task, tasks);
              const dagReady = isTaskDagReady(tasks, executions, task.taskId);
              const startGate = canStartExecuteTask(
                {
                  card,
                  taskId: task.taskId,
                  tasks,
                  executions,
                  canStartStage,
                  workflowsEnabled,
                  workspaceStrategyValid: props.workspaceStrategyValid,
                  hasModelSelection: props.modelSelection !== null,
                  unsupportedPoliciesAcknowledged: policiesAcknowledged,
                },
                messages,
              );
              const taskMissingLogs = missingLogNotes.filter((note) => note.taskId === task.taskId);
              const canRetryThisTask =
                canRetryStep &&
                (status === "failed" || status === "blocked") &&
                !violationTaskIds.has(task.taskId) &&
                card.status !== "paused" &&
                !isCompleted;
              const isHighlighted = props.highlightedTargetId === task.taskId;
              const showStartGateReason =
                !exec?.lastError &&
                !startGate.allowed &&
                startGate.reason !== null &&
                status !== "completed" &&
                status !== "skipped";
              const startGateReasonId = `execute-task-gate-${task.taskId}`;

              return (
                <li
                  key={task.taskId}
                  data-execute-task-id={task.taskId}
                  tabIndex={-1}
                  className={cn(
                    "flex flex-col gap-2 rounded-md border bg-background p-3 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                    isHighlighted ? "border-primary ring-1 ring-primary" : "border-border",
                  )}
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-mono font-semibold text-foreground">{task.taskId}</span>
                      <span className="font-medium text-foreground">{task.title}</span>
                      <Badge
                        variant={
                          status === "completed"
                            ? "success"
                            : status === "failed" || status === "blocked"
                              ? "error"
                              : status === "in_progress"
                                ? "warning"
                                : "secondary"
                        }
                        size="sm"
                      >
                        {status}
                      </Badge>
                      {status !== "completed" && status !== "skipped" ? (
                        <Badge variant={dagReady ? "outline" : "secondary"} size="sm">
                          {dagReady ? <T>DAG ready</T> : <T>Blocked by dependencies</T>}
                        </Badge>
                      ) : null}
                      {exec ? (
                        <span className="font-mono text-2xs text-muted-foreground">
                          <T>
                            attempt <Var>{exec.attempt}</Var>
                          </T>
                        </span>
                      ) : null}
                    </div>

                    <div className="flex flex-wrap items-center gap-1.5">
                      {exec?.threadId ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="outline"
                          onClick={() => props.onSelectThread("execute", exec.threadId!)}
                        >
                          <ExternalLinkIcon />
                          <T>Open thread</T>
                        </Button>
                      ) : null}
                      {canRetryThisTask ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="outline"
                          disabled={busyAction !== null}
                          onClick={() => void handleRetryTask(task.taskId)}
                        >
                          <RotateCcwIcon />
                          <T>Retry task</T>
                        </Button>
                      ) : null}
                      <Button
                        type="button"
                        size="xs"
                        disabled={!startGate.allowed || busyAction !== null}
                        aria-describedby={showStartGateReason ? startGateReasonId : undefined}
                        onClick={() => void handleStartTask(task.taskId)}
                      >
                        <PlayIcon />
                        {busyAction === `start-task:${task.taskId}` ? (
                          <T>Starting…</T>
                        ) : status === "in_progress" ? (
                          <T>Resume task</T>
                        ) : (
                          <T>Start task</T>
                        )}
                      </Button>
                    </div>
                  </div>

                  {deps.length > 0 ? (
                    <p className="text-muted-foreground">
                      <T>
                        Depends on: <Var>{deps.join(", ")}</Var>
                      </T>
                    </p>
                  ) : null}

                  <p className="font-mono text-muted-foreground">
                    <T>
                      Allowed files: <Var>{task.filesAllowed.join(", ")}</Var>
                    </T>
                  </p>

                  {task.filesForbidden && task.filesForbidden.length > 0 ? (
                    <p className="font-mono text-muted-foreground">
                      <T>
                        Forbidden files: <Var>{task.filesForbidden.join(", ")}</Var>
                      </T>
                    </p>
                  ) : null}

                  {exec?.lastError ? (
                    <p className="text-destructive-foreground">{exec.lastError}</p>
                  ) : showStartGateReason ? (
                    <p id={startGateReasonId} className="text-muted-foreground">
                      {startGate.reason}
                    </p>
                  ) : null}

                  {taskMissingLogs.map((note) => (
                    <p
                      key={`${note.relativePath}:${note.note}`}
                      data-missing-log-task={task.taskId}
                      className="text-warning-foreground"
                    >
                      <span className="font-mono">{note.relativePath}</span>
                      {note.command ? ` (${note.command})` : ""}: {note.note}
                    </p>
                  ))}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section
        data-execute-section="checks"
        aria-labelledby="execute-checks-heading"
        tabIndex={-1}
        className={cn(
          "flex flex-col gap-3 rounded-lg border bg-card p-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
          props.focusedSection === "checks" ? "border-primary ring-1 ring-primary" : undefined,
        )}
      >
        <div className="flex items-center gap-2">
          <TerminalIcon className="size-4 text-muted-foreground" />
          <h3 id="execute-checks-heading" className="text-sm font-semibold text-foreground">
            <T>Planned checks & receipts</T>
          </h3>
        </div>

        {checkReceipts.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            <T>No planned argv checks configured.</T>
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {checkReceipts.map((receipt) => {
              const stale = isCheckReceiptStale(receipt, currentSnapshotDigest);
              const isHighlighted = props.highlightedTargetId === receipt.checkId;

              return (
                <li
                  key={receipt.checkId}
                  data-check-id={receipt.checkId}
                  tabIndex={-1}
                  className={cn(
                    "flex flex-col gap-2 rounded-md border bg-background p-3 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                    isHighlighted ? "border-primary ring-1 ring-primary" : "border-border",
                  )}
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-mono font-semibold text-foreground">
                        {receipt.checkId}
                      </span>
                      <Badge
                        variant={
                          stale
                            ? "warning"
                            : receipt.status === "passed"
                              ? "success"
                              : receipt.status === "failed"
                                ? "error"
                                : "secondary"
                        }
                        size="sm"
                      >
                        {workflowBoardCheckStatusLabel(receipt.status, messages)}
                      </Badge>
                      {stale ? (
                        <Badge variant="warning" size="sm">
                          <T>Stale</T>
                        </Badge>
                      ) : null}
                      {receipt.criterionId ? (
                        <Badge variant="outline" size="sm">
                          {receipt.criterionId}
                        </Badge>
                      ) : null}
                      {receipt.exitCode !== null ? (
                        <span className="font-mono text-muted-foreground">
                          <T>
                            exit <Var>{receipt.exitCode}</Var>
                          </T>
                        </span>
                      ) : null}
                    </div>

                    <Button
                      type="button"
                      size="xs"
                      variant={receipt.status === "passed" && !stale ? "outline" : "default"}
                      disabled={
                        !canRetryStep ||
                        card.status === "paused" ||
                        isCompleted ||
                        busyAction !== null
                      }
                      onClick={() => void handleRunCheck(receipt.checkId)}
                    >
                      <PlayIcon />
                      {busyAction === `check:${receipt.checkId}` ? (
                        <T>Running…</T>
                      ) : receipt.executedAt !== null ? (
                        <T>Re-run check</T>
                      ) : (
                        <T>Run check</T>
                      )}
                    </Button>
                  </div>

                  <p className="rounded bg-muted px-2 py-1 font-mono text-2xs text-foreground">
                    {formatArgvTokens(receipt.argv)}
                  </p>

                  {receipt.productSnapshotDigest ? (
                    <p className="font-mono text-2xs text-muted-foreground">
                      <T>
                        snapshot <Var>{receipt.productSnapshotDigest}</Var>
                      </T>
                      {receipt.logRef ? (
                        <>
                          {" "}
                          · <Var>{receipt.logRef}</Var>
                        </>
                      ) : null}
                    </p>
                  ) : null}

                  {stale ? (
                    <p className="text-warning-foreground">
                      {receipt.staleReason ??
                        gt(
                          "Snapshot changed after this check ran. Re-run check to refresh receipt.",
                        )}
                    </p>
                  ) : null}

                  {missingLogNotes
                    .filter((note) => note.checkId === receipt.checkId)
                    .map((note) => (
                      <p
                        key={`${note.relativePath}:${note.note}`}
                        data-missing-log-check={receipt.checkId}
                        className="text-warning-foreground"
                      >
                        <span className="font-mono">{note.relativePath}</span>: {note.note}
                      </p>
                    ))}

                  {receipt.stdoutExcerpt || receipt.stderrExcerpt ? (
                    <details className="rounded border bg-muted/30 p-2">
                      <summary className="cursor-pointer rounded-sm font-medium text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
                        <T>Check output</T>
                      </summary>
                      {receipt.stdoutExcerpt ? (
                        <pre className="mt-1 max-h-40 overflow-auto font-mono text-2xs text-foreground">
                          {receipt.stdoutExcerpt}
                        </pre>
                      ) : null}
                      {receipt.stderrExcerpt ? (
                        <pre className="mt-1 max-h-40 overflow-auto font-mono text-2xs text-destructive-foreground">
                          {receipt.stderrExcerpt}
                        </pre>
                      ) : null}
                    </details>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <div
        data-execute-section="review"
        tabIndex={-1}
        className={cn(
          "rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
          props.focusedSection === "review" ? "ring-1 ring-primary" : undefined,
        )}
      >
        <WorkflowBoardReviewPanel
          environmentId={props.environmentId}
          card={card}
          workflowsEnabled={workflowsEnabled}
          modelSelection={props.modelSelection}
          runtimeMode={props.runtimeMode}
          interactionMode={props.interactionMode}
          workspaceStrategy={props.workspaceStrategy}
          workspaceStrategyValid={props.workspaceStrategyValid}
          highlightedTargetId={props.highlightedTargetId ?? null}
          onSelectThread={(threadId) => props.onSelectThread("review", threadId)}
          {...(props.onStageStarted !== undefined ? { onStageStarted: props.onStageStarted } : {})}
          onActionError={props.onActionError}
          onActionNotice={props.onActionNotice}
        />
      </div>

      <WorkflowBoardCardDiffViewer
        diff={props.cardDiff}
        loading={props.cardDiffLoading}
        error={props.cardDiffError}
        onRefresh={props.onRefreshDiff}
      />
    </div>
  );
}

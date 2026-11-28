import { useAtomValue } from "@effect/atom-react";
import {
  workflowBoardErrorMessage,
  workflowBoardReviewVerdictLabel,
} from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  type EnvironmentId,
  type ModelSelection,
  type ProviderInteractionMode,
  type RuntimeMode,
  type ThreadId,
  type WorkflowBoardFindingResolveDisposition,
  type WorkflowBoardReviewFinding,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import {
  ExternalLinkIcon,
  PlayIcon,
  RotateCcwIcon,
  ShieldCheckIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useId, useState } from "react";

import { cn } from "../../lib/utils";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  canStartIndependentReview,
  isReviewReportStale,
  normalizeWorkspaceStrategy,
} from "./workflowBoard.logic";

interface FindingDraft {
  readonly disposition: WorkflowBoardFindingResolveDisposition;
  readonly reason: string;
}

const DISPOSITIONS: readonly WorkflowBoardFindingResolveDisposition[] = [
  "fix",
  "accept_risk",
  "dismiss",
  "defer",
];

function dispositionLabel(
  disposition: WorkflowBoardFindingResolveDisposition,
  gt: ReturnType<typeof useGT>,
): string {
  switch (disposition) {
    case "fix":
      return gt("Fix in code", { $context: "workflow board review finding disposition" });
    case "accept_risk":
      return gt("Accept risk", { $context: "workflow board review finding disposition" });
    case "dismiss":
      return gt("Dismiss", { $context: "workflow board review finding disposition" });
    case "defer":
      return gt("Defer", { $context: "workflow board review finding disposition" });
  }
}

export function WorkflowBoardReviewPanel(props: {
  readonly environmentId: EnvironmentId;
  readonly card: WorkflowCardDetail;
  readonly workflowsEnabled: boolean;
  readonly modelSelection: ModelSelection | null;
  readonly runtimeMode: RuntimeMode;
  readonly interactionMode: ProviderInteractionMode;
  readonly workspaceStrategy: WorkflowCardDetail["workspaceStrategy"];
  readonly workspaceStrategyValid: boolean;
  readonly highlightedTargetId?: string | null;
  readonly onSelectThread: (threadId: ThreadId) => void;
  readonly onStageStarted?: (threadId: ThreadId) => void;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const reviewGateReasonId = useId();
  const reviewStaleTitleId = useId();
  const { card } = props;

  const canStartStage = useAtomValue(workflowBoard.startStage.permissionAtom(props.environmentId));
  const canRetryStep = useAtomValue(workflowBoard.retryStep.permissionAtom(props.environmentId));
  const canResolveFinding = useAtomValue(
    workflowBoard.resolveFinding.permissionAtom(props.environmentId),
  );

  const startStage = useAtomCommand(workflowBoard.startStage, {
    label: "workflow board start stage",
    reportFailure: false,
  });
  const retryStep = useAtomCommand(workflowBoard.retryStep, {
    label: "workflow board retry step",
    reportFailure: false,
  });
  const resolveFinding = useAtomCommand(workflowBoard.resolveFinding, {
    label: "workflow board resolve finding",
    reportFailure: false,
  });

  const [drafts, setDrafts] = useState<Readonly<Record<string, FindingDraft>>>({});
  const [validationErrors, setValidationErrors] = useState<Readonly<Record<string, string>>>({});
  const [busyAction, setBusyAction] = useState<string | null>(null);

  const tasks = card.plan?.tasks ?? card.tasks ?? [];
  const executions = card.taskExecutions ?? [];
  const reviewGate = canStartIndependentReview(
    {
      card,
      tasks,
      executions,
      canStartStage,
      workflowsEnabled: props.workflowsEnabled,
      workspaceStrategyValid: props.workspaceStrategyValid,
      hasModelSelection: props.modelSelection !== null,
    },
    messages,
  );

  const reviewReport = card.reviewReport ?? null;
  const currentSnapshotDigest = card.shipReadiness?.productSnapshotDigest ?? null;
  const reviewStale =
    reviewReport !== null && isReviewReportStale(reviewReport, currentSnapshotDigest);
  const canRetryReview =
    canRetryStep &&
    reviewReport !== null &&
    (reviewReport.status === "failed" || reviewReport.verdict === "failed") &&
    card.status !== "paused" &&
    card.status !== "completed";

  const handleStartReview = async () => {
    if (!reviewGate.allowed || props.modelSelection === null || busyAction !== null) return;
    setBusyAction("start-review");
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await startStage({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stage: "review",
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
          gt("Could not start independent review."),
      );
      return;
    }
    props.onSelectThread(result.value.threadId);
    props.onStageStarted?.(result.value.threadId);
    props.onActionNotice(gt("Independent review launched in a fresh thread."));
  };

  const handleRetryReview = async () => {
    if (!canRetryReview || busyAction !== null) return;
    setBusyAction("retry-review");
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await retryStep({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stepKind: "review",
      },
    });
    setBusyAction(null);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not retry independent review."),
      );
      return;
    }
    props.onActionNotice(gt("Review reset for re-run."));
  };

  const draftFor = (finding: WorkflowBoardReviewFinding): FindingDraft =>
    drafts[finding.id] ?? {
      disposition:
        finding.disposition !== "unresolved" ? finding.disposition : ("accept_risk" as const),
      reason: finding.resolutionNote ?? "",
    };

  const updateDraft = (findingId: string, next: FindingDraft) => {
    setDrafts((current) => ({ ...current, [findingId]: next }));
    setValidationErrors((current) => {
      if (!(findingId in current)) return current;
      const copy = { ...current };
      delete copy[findingId];
      return copy;
    });
  };

  const handleResolveFinding = async (finding: WorkflowBoardReviewFinding) => {
    if (!canResolveFinding || reviewReport === null || busyAction !== null) return;
    const draft = draftFor(finding);
    const trimmedReason = draft.reason.trim();
    if (
      (draft.disposition === "dismiss" || draft.disposition === "accept_risk") &&
      trimmedReason.length === 0
    ) {
      setValidationErrors((current) => ({
        ...current,
        [finding.id]: gt("A reason is required when dismissing or accepting risk for a finding."),
      }));
      return;
    }

    setBusyAction(`resolve:${finding.id}`);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await resolveFinding({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        reviewId: reviewReport.reviewId,
        findingId: finding.id,
        disposition: draft.disposition,
        ...(trimmedReason.length > 0 ? { reason: trimmedReason } : {}),
      },
    });
    setBusyAction(null);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not resolve review finding."),
      );
      return;
    }
    props.onActionNotice(gt("Updated finding {id}.", { id: finding.id }));
  };

  return (
    <section
      aria-labelledby="execute-review-heading"
      className="flex flex-col gap-3 rounded-lg border bg-card p-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <ShieldCheckIcon className="size-4 text-muted-foreground" />
          <h3 id="execute-review-heading" className="text-sm font-semibold text-foreground">
            <T>Independent review</T>
          </h3>
          {reviewReport !== null ? (
            <Badge
              variant={
                reviewStale
                  ? "warning"
                  : reviewReport.verdict === "pass"
                    ? "success"
                    : reviewReport.verdict === "failed"
                      ? "error"
                      : "secondary"
              }
              size="sm"
            >
              {workflowBoardReviewVerdictLabel(reviewReport.verdict ?? "pending", messages)}
            </Badge>
          ) : null}
          {reviewReport?.status === "historical" ? (
            <Badge variant="secondary" size="sm">
              <T>Historical</T>
            </Badge>
          ) : null}
          {reviewStale ? (
            <Badge variant="warning" size="sm">
              <T>Stale</T>
            </Badge>
          ) : null}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {reviewReport?.threadId ? (
            <Button
              type="button"
              size="xs"
              variant="outline"
              onClick={() => props.onSelectThread(reviewReport.threadId!)}
            >
              <ExternalLinkIcon />
              <T>Open review thread</T>
            </Button>
          ) : null}
          {canRetryReview ? (
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={busyAction !== null}
              onClick={() => void handleRetryReview()}
            >
              <RotateCcwIcon />
              <T>Retry review</T>
            </Button>
          ) : null}
          <Button
            type="button"
            size="xs"
            disabled={!reviewGate.allowed || busyAction !== null}
            aria-describedby={
              !reviewGate.allowed && reviewGate.reason !== null ? reviewGateReasonId : undefined
            }
            onClick={() => void handleStartReview()}
          >
            <PlayIcon />
            {busyAction === "start-review" ? (
              <T>Launching review…</T>
            ) : (
              <T>Start independent review</T>
            )}
          </Button>
        </div>
      </div>

      {!reviewGate.allowed && reviewGate.reason !== null ? (
        <p id={reviewGateReasonId} className="text-xs text-muted-foreground">
          {reviewGate.reason}
        </p>
      ) : (
        <p className="text-xs text-muted-foreground">
          <T>
            Launches in a fresh thread with no prior execution turn bias against the current product
            snapshot.
          </T>
        </p>
      )}

      {reviewStale ? (
        <Alert variant="warning" role="region" aria-labelledby={reviewStaleTitleId}>
          <TriangleAlertIcon />
          <AlertTitle id={reviewStaleTitleId}>
            <T>Independent review is stale</T>
          </AlertTitle>
          <AlertDescription>
            {reviewReport?.staleReason ??
              gt(
                "The product snapshot or plan changed after this review ran. Start a fresh independent review.",
              )}
          </AlertDescription>
        </Alert>
      ) : null}

      {reviewReport === null ? (
        <p className="text-xs text-muted-foreground">
          <T>No independent review recorded yet.</T>
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
            <span className="font-mono">{reviewReport.reviewId}</span>
            <span>
              <T>
                status <Var>{reviewReport.status}</Var>
              </T>
            </span>
            {reviewReport.productSnapshotDigest ? (
              <span className="font-mono">
                <T>
                  snapshot <Var>{reviewReport.productSnapshotDigest}</Var>
                </T>
              </span>
            ) : null}
          </div>
          {reviewReport.summary ? (
            <p className="text-xs text-foreground">{reviewReport.summary}</p>
          ) : null}

          {reviewReport.findings.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              <T>No findings reported.</T>
            </p>
          ) : (
            <ul className="flex flex-col gap-3">
              {reviewReport.findings.map((finding) => {
                const draft = draftFor(finding);
                const validationError = validationErrors[finding.id] ?? null;
                const isHighlighted = props.highlightedTargetId === finding.id;
                const findingBlocked =
                  finding.status === "open" ||
                  (finding.status === "resolved" && finding.disposition === "fix");

                return (
                  <li
                    key={finding.id}
                    data-review-finding-id={finding.id}
                    tabIndex={-1}
                    className={cn(
                      "flex flex-col gap-2 rounded-md border bg-background p-3 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                      isHighlighted ? "border-primary ring-1 ring-primary" : "border-border",
                    )}
                  >
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono font-semibold text-foreground">
                          {finding.id}
                        </span>
                        <Badge
                          variant={
                            finding.severity === "critical" || finding.severity === "major"
                              ? "error"
                              : "secondary"
                          }
                          size="sm"
                        >
                          {finding.severity}
                        </Badge>
                        <Badge variant={findingBlocked ? "warning" : "success"} size="sm">
                          {finding.disposition}
                        </Badge>
                        {finding.criterionId ? (
                          <Badge variant="outline" size="sm">
                            {finding.criterionId}
                          </Badge>
                        ) : null}
                      </div>
                      {finding.file ? (
                        <span className="font-mono text-muted-foreground">
                          {finding.file}
                          {finding.line !== null ? `:${finding.line}` : ""}
                        </span>
                      ) : null}
                    </div>

                    <p className="text-foreground">{finding.message}</p>

                    {finding.resolutionNote ? (
                      <p className="text-muted-foreground">
                        <T>
                          Resolution note: <Var>{finding.resolutionNote}</Var>
                        </T>
                      </p>
                    ) : null}

                    {!reviewStale && card.status !== "completed" ? (
                      <div className="mt-1 flex flex-col gap-2 border-t pt-2">
                        <div className="flex flex-wrap items-center gap-2">
                          <Select
                            value={draft.disposition}
                            disabled={!canResolveFinding || busyAction !== null}
                            onValueChange={(value) => {
                              if (
                                value === "fix" ||
                                value === "accept_risk" ||
                                value === "dismiss" ||
                                value === "defer"
                              ) {
                                updateDraft(finding.id, { ...draft, disposition: value });
                              }
                            }}
                          >
                            <SelectTrigger
                              size="sm"
                              aria-label={gt("Disposition for {id}", { id: finding.id })}
                            >
                              <SelectValue>{dispositionLabel(draft.disposition, gt)}</SelectValue>
                            </SelectTrigger>
                            <SelectPopup>
                              {DISPOSITIONS.map((value) => (
                                <SelectItem key={value} value={value}>
                                  {dispositionLabel(value, gt)}
                                </SelectItem>
                              ))}
                            </SelectPopup>
                          </Select>
                          <Button
                            type="button"
                            size="xs"
                            variant="outline"
                            disabled={!canResolveFinding || busyAction !== null}
                            onClick={() => void handleResolveFinding(finding)}
                          >
                            {busyAction === `resolve:${finding.id}` ? (
                              <T>Saving…</T>
                            ) : (
                              <T>Resolve finding</T>
                            )}
                          </Button>
                        </div>

                        <Textarea
                          rows={2}
                          aria-label={gt("Resolution reason for {id}", { id: finding.id })}
                          aria-invalid={validationError !== null || undefined}
                          aria-describedby={
                            validationError !== null
                              ? `review-finding-error-${finding.id}`
                              : undefined
                          }
                          placeholder={gt(
                            "Explain disposition (required for Dismiss or Accept risk)...",
                          )}
                          value={draft.reason}
                          disabled={!canResolveFinding || busyAction !== null}
                          onChange={(event) =>
                            updateDraft(finding.id, { ...draft, reason: event.target.value })
                          }
                        />
                        {validationError !== null ? (
                          <p
                            id={`review-finding-error-${finding.id}`}
                            role="alert"
                            className="text-xs text-destructive-foreground"
                          >
                            {validationError}
                          </p>
                        ) : null}
                      </div>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

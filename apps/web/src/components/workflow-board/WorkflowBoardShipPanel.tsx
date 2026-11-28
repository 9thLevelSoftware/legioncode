import { useAtomValue } from "@effect/atom-react";
import { scopeThreadRef } from "@legioncode/client-runtime/environment";
import {
  canApproveWorkflowBoardDelivery,
  isWorkflowBoardCardShipped,
  workflowBoardAcEvidenceStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardLatestStageRun,
  workflowBoardStageLabel,
} from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  type EnvironmentId,
  type ProjectId,
  type ThreadId,
  type WorkflowBoardFixPointer,
  type WorkflowBoardGetCardDiffResult,
  type WorkflowBoardManualAcceptanceVerdict,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import {
  AlertTriangleIcon,
  ArrowRightIcon,
  CheckCircle2Icon,
  ClipboardCheckIcon,
  GitBranchIcon,
  RocketIcon,
} from "lucide-react";
import { useState } from "react";

import { cn } from "../../lib/utils";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { GitActionsControl } from "../GitActionsControl";
import { Alert, AlertDescription } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import { isAcObservationStale } from "./workflowBoard.logic";
import { WorkflowBoardCardDiffViewer } from "./WorkflowBoardCardDiffViewer";

interface CriterionDraft {
  readonly verdict: WorkflowBoardManualAcceptanceVerdict;
  readonly note: string;
  readonly evidenceRef: string;
}

const VERDICTS: readonly WorkflowBoardManualAcceptanceVerdict[] = [
  "pass",
  "fail",
  "not-applicable",
];

function verdictLabel(
  verdict: WorkflowBoardManualAcceptanceVerdict,
  gt: ReturnType<typeof useGT>,
): string {
  switch (verdict) {
    case "pass":
      return gt("Pass", { $context: "workflow board manual acceptance verdict" });
    case "fail":
      return gt("Fail", { $context: "workflow board manual acceptance verdict" });
    case "not-applicable":
      return gt("Not applicable", { $context: "workflow board manual acceptance verdict" });
  }
}

export function WorkflowBoardShipPanel(props: {
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly workspaceRoot: string;
  readonly card: WorkflowCardDetail;
  readonly cardDiff: WorkflowBoardGetCardDiffResult | null;
  readonly cardDiffLoading: boolean;
  readonly cardDiffError: string | null;
  readonly displayedThreadId: ThreadId | null;
  readonly highlightedTargetId?: string | null;
  readonly onRefreshDiff: () => void;
  readonly onRefreshCard: () => void;
  readonly onNavigateToPointer: (pointer: WorkflowBoardFixPointer) => void;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;

  const canRecordAcceptance = useAtomValue(
    workflowBoard.recordManualAcceptance.permissionAtom(props.environmentId),
  );
  const canApproveDeliveryPerm = useAtomValue(
    workflowBoard.approveDelivery.permissionAtom(props.environmentId),
  );

  const recordManualAcceptance = useAtomCommand(workflowBoard.recordManualAcceptance, {
    label: "workflow board record manual acceptance",
    reportFailure: false,
  });
  const approveDelivery = useAtomCommand(workflowBoard.approveDelivery, {
    label: "workflow board approve delivery",
    reportFailure: false,
  });

  const [criterionDrafts, setCriterionDrafts] = useState<Readonly<Record<string, CriterionDraft>>>(
    {},
  );
  const [criterionErrors, setCriterionErrors] = useState<Readonly<Record<string, string>>>({});
  const [deliveryNote, setDeliveryNote] = useState("");
  const [busyAction, setBusyAction] = useState<string | null>(null);

  const shipped = isWorkflowBoardCardShipped(card);
  const currentSnapshotDigest =
    card.shipReadiness?.productSnapshotDigest ?? props.cardDiff?.productSnapshotDigest ?? null;
  const blockers = card.shipReadiness?.blockers ?? [];
  const observationsByCriterion = new Map(
    (card.acObservations ?? []).map((obs) => [obs.criterionId, obs]),
  );
  const mappingsByCriterion = new Map(
    (card.plan?.acMappings ?? card.acMappings ?? []).map((mapping) => [
      mapping.criterionId,
      mapping,
    ]),
  );

  const canApprove = canApproveWorkflowBoardDelivery({
    detail: card,
    canOperate: canApproveDeliveryPerm,
  });

  const latestExecuteRun = workflowBoardLatestStageRun(card, "execute");
  const latestExecuteWorktree =
    (card.taskExecutions ?? []).find((exec) => exec.worktreePath !== null)?.worktreePath ??
    latestExecuteRun?.worktreePath ??
    null;
  const gitCwd = props.cardDiff?.worktreePath ?? latestExecuteWorktree ?? props.workspaceRoot;
  const preferredFilePaths =
    props.cardDiff && props.cardDiff.files.length > 0
      ? props.cardDiff.files.map((file) => file.path)
      : undefined;

  const draftForCriterion = (criterionId: string): CriterionDraft => {
    const existing = criterionDrafts[criterionId];
    if (existing !== undefined) return existing;
    const obs = observationsByCriterion.get(criterionId);
    return {
      verdict:
        obs?.status === "fail"
          ? "fail"
          : obs?.status === "not_applicable"
            ? "not-applicable"
            : "pass",
      note: obs?.observationNote ?? "",
      evidenceRef: obs?.evidenceRef ?? "",
    };
  };

  const updateCriterionDraft = (criterionId: string, next: CriterionDraft) => {
    setCriterionDrafts((current) => ({ ...current, [criterionId]: next }));
    setCriterionErrors((current) => {
      if (!(criterionId in current)) return current;
      const copy = { ...current };
      delete copy[criterionId];
      return copy;
    });
  };

  const handleRecordAcceptance = async (criterionId: string) => {
    if (!canRecordAcceptance || shipped || busyAction !== null) return;
    const draft = draftForCriterion(criterionId);
    const trimmedNote = draft.note.trim();
    if (
      (draft.verdict === "fail" || draft.verdict === "not-applicable") &&
      trimmedNote.length === 0
    ) {
      setCriterionErrors((current) => ({
        ...current,
        [criterionId]: gt("A note is required when recording Fail or Not applicable."),
      }));
      return;
    }

    setBusyAction(`ac:${criterionId}`);
    props.onActionError(null);
    props.onActionNotice(null);

    const trimmedEvidenceRef = draft.evidenceRef.trim();
    const result = await recordManualAcceptance({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        criterionId,
        verdict: draft.verdict,
        ...(trimmedNote.length > 0 ? { note: trimmedNote } : {}),
        ...(trimmedEvidenceRef.length > 0 ? { evidenceRef: trimmedEvidenceRef } : {}),
        ...(currentSnapshotDigest !== null
          ? { expectedProductSnapshotDigest: currentSnapshotDigest }
          : {}),
      },
    });
    setBusyAction(null);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not record acceptance observation."),
      );
      return;
    }
    props.onActionNotice(gt("Recorded observation for {criterionId}.", { criterionId }));
  };

  const handleApproveDelivery = async () => {
    if (!canApprove || busyAction !== null) return;
    setBusyAction("approve-delivery");
    props.onActionError(null);
    props.onActionNotice(null);

    const trimmedNote = deliveryNote.trim();
    const result = await approveDelivery({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        ...(currentSnapshotDigest !== null
          ? { expectedProductSnapshotDigest: currentSnapshotDigest }
          : {}),
        ...(trimmedNote.length > 0 ? { note: trimmedNote } : {}),
      },
    });
    setBusyAction(null);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not approve delivery."),
      );
      return;
    }
    setDeliveryNote("");
    props.onActionNotice(gt("Delivery approved. Card marked shipped without auto-commit."));
  };

  const deliveryReceipt = card.deliveryReceipt ?? card.shipReadiness?.deliveryReceipt ?? null;

  return (
    <div className="flex flex-col gap-5">
      {shipped && deliveryReceipt !== null ? (
        <section
          data-ship-delivery-receipt={deliveryReceipt.id}
          aria-labelledby="ship-receipt-heading"
          className="flex flex-col gap-3 rounded-lg border border-success/60 bg-success/5 p-4"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <CheckCircle2Icon className="size-4 text-success" />
              <h3 id="ship-receipt-heading" className="text-sm font-semibold text-foreground">
                <T>Delivery receipt</T>
              </h3>
            </div>
            <Badge variant="success" size="sm">
              {workflowBoardDeliveryStatusLabel(deliveryReceipt.status, messages)}
            </Badge>
          </div>
          <div className="grid gap-2 text-xs sm:grid-cols-2">
            <div>
              <span className="text-muted-foreground">
                <T>Receipt ID</T>
              </span>
              <p className="font-mono text-foreground">{deliveryReceipt.id}</p>
            </div>
            <div>
              <span className="text-muted-foreground">
                <T>Product snapshot</T>
              </span>
              <p className="font-mono text-foreground">{deliveryReceipt.productSnapshotDigest}</p>
            </div>
            <div>
              <span className="text-muted-foreground">
                <T>Approved by</T>
              </span>
              <p className="text-foreground">
                {deliveryReceipt.approvedBy} · {deliveryReceipt.approvedAt}
              </p>
            </div>
            <div>
              <span className="text-muted-foreground">
                <T>AC summary</T>
              </span>
              <p className="text-foreground">{deliveryReceipt.acVerdictsSummary}</p>
            </div>
          </div>
          {deliveryReceipt.commitSha ? (
            <p className="font-mono text-2xs text-muted-foreground">
              <T>
                commit <Var>{deliveryReceipt.commitSha}</Var>
              </T>
            </p>
          ) : null}
        </section>
      ) : null}

      <section
        data-ship-section="readiness"
        tabIndex={-1}
        aria-labelledby="ship-readiness-heading"
        className="flex flex-col gap-3 rounded-lg border bg-card p-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <RocketIcon className="size-4 text-muted-foreground" />
            <h3 id="ship-readiness-heading" className="text-sm font-semibold text-foreground">
              <T>Delivery readiness & blockers</T>
            </h3>
            <Badge
              variant={
                shipped
                  ? "success"
                  : card.shipReadiness?.ready
                    ? "success"
                    : blockers.length > 0
                      ? "error"
                      : "secondary"
              }
              size="sm"
            >
              {workflowBoardDeliveryStatusLabel(
                card.shipReadiness?.status ?? card.deliveryStatus,
                messages,
              )}
            </Badge>
          </div>
          {currentSnapshotDigest !== null ? (
            <span className="font-mono text-2xs text-muted-foreground">
              <T>
                snapshot <Var>{currentSnapshotDigest}</Var>
              </T>
            </span>
          ) : null}
        </div>

        {blockers.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            {shipped ? (
              <T>This card has been approved for delivery.</T>
            ) : (
              <T>All checks, independent review findings, and acceptance criteria are current.</T>
            )}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {blockers.map((blocker) => (
              <li
                key={blocker.id}
                data-ship-blocker-id={blocker.id}
                className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2.5 text-xs"
              >
                <div className="flex min-w-0 flex-1 items-start gap-2">
                  <AlertTriangleIcon className="mt-0.5 size-3.5 shrink-0 text-destructive-foreground" />
                  <div className="min-w-0">
                    <p
                      id={`ship-blocker-msg-${blocker.id}`}
                      className="font-medium text-foreground"
                    >
                      {blocker.message}
                    </p>
                    <p className="font-mono text-2xs text-muted-foreground">
                      {workflowBoardStageLabel(blocker.pointer.stage, messages)} ·{" "}
                      {blocker.pointer.tab}
                      {blocker.pointer.targetId ? ` · ${blocker.pointer.targetId}` : ""}
                    </p>
                  </div>
                </div>
                <Button
                  type="button"
                  size="xs"
                  variant="outline"
                  aria-describedby={`ship-blocker-msg-${blocker.id}`}
                  onClick={() => props.onNavigateToPointer(blocker.pointer)}
                >
                  <T>Go to fix</T>
                  <ArrowRightIcon />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <WorkflowBoardCardDiffViewer
        diff={props.cardDiff}
        loading={props.cardDiffLoading}
        error={props.cardDiffError}
        onRefresh={props.onRefreshDiff}
      />

      <section
        data-ship-section="acceptance"
        tabIndex={-1}
        aria-labelledby="ship-acceptance-heading"
        className="flex flex-col gap-3 rounded-lg border bg-card p-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      >
        <div className="flex items-center gap-2">
          <ClipboardCheckIcon className="size-4 text-muted-foreground" />
          <h3 id="ship-acceptance-heading" className="text-sm font-semibold text-foreground">
            <T>Acceptance criteria verification</T>
          </h3>
        </div>

        {card.spec.acceptanceCriteria.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            <T>No acceptance criteria defined in the spec.</T>
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {card.spec.acceptanceCriteria.map((criterion) => {
              const obs = observationsByCriterion.get(criterion.id) ?? null;
              const mapping = mappingsByCriterion.get(criterion.id) ?? null;
              const draft = draftForCriterion(criterion.id);
              const validationError = criterionErrors[criterion.id] ?? null;
              const stale = obs !== null && isAcObservationStale(obs, currentSnapshotDigest);
              const isHighlighted = props.highlightedTargetId === criterion.id;

              return (
                <li
                  key={criterion.id}
                  data-ac-criterion-id={criterion.id}
                  tabIndex={-1}
                  className={cn(
                    "flex flex-col gap-2 rounded-md border bg-background p-3 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                    isHighlighted ? "border-primary ring-1 ring-primary" : "border-border",
                  )}
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-mono font-semibold text-foreground">
                        {criterion.id}
                      </span>
                      {mapping ? (
                        <Badge variant="outline" size="sm">
                          {mapping.method}
                        </Badge>
                      ) : null}
                      {obs ? (
                        <Badge
                          variant={
                            stale
                              ? "warning"
                              : obs.status === "pass" || obs.status === "not_applicable"
                                ? "success"
                                : obs.status === "fail"
                                  ? "error"
                                  : "secondary"
                          }
                          size="sm"
                        >
                          {workflowBoardAcEvidenceStatusLabel(obs.status, messages)}
                        </Badge>
                      ) : null}
                      {stale ? (
                        <Badge variant="warning" size="sm">
                          <T>Stale</T>
                        </Badge>
                      ) : null}
                    </div>
                    {obs?.productSnapshotDigest ? (
                      <span className="font-mono text-2xs text-muted-foreground">
                        <T>
                          snapshot <Var>{obs.productSnapshotDigest}</Var>
                        </T>
                      </span>
                    ) : null}
                  </div>

                  <p className="text-foreground">{criterion.text}</p>

                  {obs?.procedure ? (
                    <p className="text-muted-foreground">
                      <T>
                        Procedure: <Var>{obs.procedure}</Var>
                      </T>
                    </p>
                  ) : null}

                  {stale ? (
                    <Alert variant="warning" role="note">
                      <AlertDescription>
                        {obs.staleReason ??
                          gt(
                            "Product snapshot changed after this observation. Record updated verification.",
                          )}
                      </AlertDescription>
                    </Alert>
                  ) : null}

                  {!shipped ? (
                    <div className="mt-1 flex flex-col gap-2 border-t pt-2">
                      <div className="flex flex-wrap items-center gap-2">
                        <Select
                          value={draft.verdict}
                          disabled={!canRecordAcceptance || busyAction !== null}
                          onValueChange={(value) => {
                            if (
                              value === "pass" ||
                              value === "fail" ||
                              value === "not-applicable"
                            ) {
                              updateCriterionDraft(criterion.id, { ...draft, verdict: value });
                            }
                          }}
                        >
                          <SelectTrigger
                            size="sm"
                            aria-label={gt("Verdict for {id}", { id: criterion.id })}
                          >
                            <SelectValue>{verdictLabel(draft.verdict, gt)}</SelectValue>
                          </SelectTrigger>
                          <SelectPopup>
                            {VERDICTS.map((verdict) => (
                              <SelectItem key={verdict} value={verdict}>
                                {verdictLabel(verdict, gt)}
                              </SelectItem>
                            ))}
                          </SelectPopup>
                        </Select>

                        <Input
                          aria-label={gt("Evidence reference for {id}", { id: criterion.id })}
                          placeholder={gt("Optional evidence ref / URL...")}
                          value={draft.evidenceRef}
                          disabled={!canRecordAcceptance || busyAction !== null}
                          onChange={(event) =>
                            updateCriterionDraft(criterion.id, {
                              ...draft,
                              evidenceRef: event.target.value,
                            })
                          }
                          className="max-w-xs"
                        />

                        <Button
                          type="button"
                          size="xs"
                          variant="outline"
                          disabled={!canRecordAcceptance || busyAction !== null}
                          onClick={() => void handleRecordAcceptance(criterion.id)}
                        >
                          {busyAction === `ac:${criterion.id}` ? (
                            <T>Saving…</T>
                          ) : (
                            <T>Record observation</T>
                          )}
                        </Button>
                      </div>

                      <Textarea
                        rows={2}
                        aria-label={gt("Observation note for {id}", { id: criterion.id })}
                        aria-invalid={validationError !== null || undefined}
                        aria-describedby={
                          validationError !== null ? `ship-ac-error-${criterion.id}` : undefined
                        }
                        placeholder={gt(
                          "Observation note (required for Fail or Not applicable)...",
                        )}
                        value={draft.note}
                        disabled={!canRecordAcceptance || busyAction !== null}
                        onChange={(event) =>
                          updateCriterionDraft(criterion.id, { ...draft, note: event.target.value })
                        }
                      />

                      {validationError !== null ? (
                        <p
                          id={`ship-ac-error-${criterion.id}`}
                          role="alert"
                          className="text-xs text-destructive-foreground"
                        >
                          {validationError}
                        </p>
                      ) : null}
                    </div>
                  ) : obs?.observationNote ? (
                    <p className="text-muted-foreground">{obs.observationNote}</p>
                  ) : null}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {!shipped ? (
        <section
          aria-labelledby="ship-approve-heading"
          className="flex flex-col gap-3 rounded-lg border bg-card p-4"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <h3 id="ship-approve-heading" className="text-sm font-semibold text-foreground">
                <T>Approve delivery</T>
              </h3>
              <p className="text-xs text-muted-foreground">
                <T>
                  Records an authenticated human delivery receipt against the tested snapshot
                  without auto-committing changes.
                </T>
              </p>
            </div>
            <Button
              type="button"
              size="sm"
              disabled={!canApprove || busyAction !== null}
              onClick={() => void handleApproveDelivery()}
            >
              <CheckCircle2Icon />
              {busyAction === "approve-delivery" ? (
                <T>Approving delivery…</T>
              ) : (
                <T>Approve delivery</T>
              )}
            </Button>
          </div>

          <Textarea
            rows={2}
            aria-label={gt("Delivery approval note")}
            placeholder={gt("Optional delivery note...")}
            value={deliveryNote}
            disabled={!canApprove || busyAction !== null}
            onChange={(event) => setDeliveryNote(event.target.value)}
          />
        </section>
      ) : null}

      <section
        aria-labelledby="ship-git-heading"
        className="flex flex-col gap-3 rounded-lg border bg-card p-4"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <GitBranchIcon className="size-4 text-muted-foreground" />
            <div>
              <h3 id="ship-git-heading" className="text-sm font-semibold text-foreground">
                <T>Source control delivery actions</T>
              </h3>
              <p className="text-xs text-muted-foreground">
                <T>
                  Commit, Push, and Create PR are individually selected actions. Nothing is
                  committed automatically.
                </T>
              </p>
            </div>
          </div>

          <GitActionsControl
            environmentId={props.environmentId}
            target={{
              key: `board:${card.cardId}:${gitCwd}`,
              cwd: gitCwd,
              name: card.title,
            }}
            threadRef={
              props.displayedThreadId
                ? scopeThreadRef(props.environmentId, props.displayedThreadId)
                : null
            }
            owner={{
              projectId: props.projectId,
              ...(props.displayedThreadId ? { threadId: props.displayedThreadId } : {}),
            }}
            {...(preferredFilePaths !== undefined ? { preferredFilePaths } : {})}
            allowStackedQuickAction={false}
            showChanges={false}
            onActionSuccess={() => {
              props.onRefreshDiff();
              props.onRefreshCard();
            }}
          />
        </div>
      </section>
    </div>
  );
}

import { useAtomValue } from "@effect/atom-react";
import {
  workflowBoardErrorMessage,
  workflowBoardStageLabel,
} from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  type EnvironmentId,
  type WorkflowBoardCardSummary,
  type WorkflowBoardReturnTargetStage,
  type WorkflowBoardStage,
  type WorkflowCardId,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { useState } from "react";

import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription } from "../ui/alert";
import {
  AlertDialog,
  AlertDialogClose,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogPopup,
  AlertDialogTitle,
} from "../ui/alert-dialog";
import { Button } from "../ui/button";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  validReturnTargetStages,
  type WorkflowBoardCrossColumnTransition,
} from "./workflowBoard.logic";

export interface WorkflowBoardPendingTransition {
  readonly card: WorkflowBoardCardSummary;
  readonly transition: WorkflowBoardCrossColumnTransition;
}

export function WorkflowBoardTransitionDialog(props: {
  readonly pending: WorkflowBoardPendingTransition | null;
  readonly environmentId: EnvironmentId;
  readonly onClose: () => void;
  readonly onCompleted: (nextStage: WorkflowBoardStage) => void;
  readonly onInspectShip?: (cardId: WorkflowCardId) => void;
}) {
  const gt = useGT();
  const messages = useMessages();

  const canApproveSpec = useAtomValue(
    workflowBoard.approveSpec.permissionAtom(props.environmentId),
  );
  const canApprovePlan = useAtomValue(
    workflowBoard.approvePlan.permissionAtom(props.environmentId),
  );
  const canApproveDelivery = useAtomValue(
    workflowBoard.approveDelivery.permissionAtom(props.environmentId),
  );
  const canReturn = useAtomValue(
    workflowBoard.returnForRevision.permissionAtom(props.environmentId),
  );

  const approveSpec = useAtomCommand(workflowBoard.approveSpec, {
    label: "workflow board approve spec",
    reportFailure: false,
  });
  const approvePlan = useAtomCommand(workflowBoard.approvePlan, {
    label: "workflow board approve plan",
    reportFailure: false,
  });
  const approveDelivery = useAtomCommand(workflowBoard.approveDelivery, {
    label: "workflow board approve delivery",
    reportFailure: false,
  });
  const returnForRevision = useAtomCommand(workflowBoard.returnForRevision, {
    label: "workflow board return for revision",
    reportFailure: false,
  });

  const [reason, setReason] = useState("");
  const [deliveryNote, setDeliveryNote] = useState("");
  const [overrideTarget, setOverrideTarget] = useState<WorkflowBoardReturnTargetStage | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const pending = props.pending;
  const resetAndClose = () => {
    setReason("");
    setDeliveryNote("");
    setOverrideTarget(null);
    setErrorMessage(null);
    props.onClose();
  };

  if (pending === null) {
    return null;
  }

  const { card, transition } = pending;
  const returnTargets = validReturnTargetStages(card.stage);
  const activeReturnTarget: WorkflowBoardReturnTargetStage =
    transition.kind === "return_for_revision"
      ? (overrideTarget ?? transition.targetStage)
      : (returnTargets[0] ?? "spec");

  const permitted =
    transition.kind === "approve_spec"
      ? canApproveSpec
      : transition.kind === "approve_plan"
        ? canApprovePlan
        : transition.kind === "approve_delivery"
          ? canApproveDelivery
          : canReturn;

  const canConfirm =
    permitted &&
    !submitting &&
    (transition.kind !== "return_for_revision" || reason.trim().length > 0);

  const handleConfirm = async () => {
    if (!canConfirm) return;
    setSubmitting(true);
    setErrorMessage(null);

    if (transition.kind === "approve_spec") {
      const result = await approveSpec({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
        },
      });
      setSubmitting(false);
      if (result._tag === "Failure") {
        setErrorMessage(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not approve spec."),
        );
        return;
      }
      resetAndClose();
      props.onCompleted("plan");
      return;
    }

    if (transition.kind === "approve_plan") {
      const result = await approvePlan({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
        },
      });
      setSubmitting(false);
      if (result._tag === "Failure") {
        setErrorMessage(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not approve plan."),
        );
        return;
      }
      resetAndClose();
      props.onCompleted("execute");
      return;
    }

    if (transition.kind === "approve_delivery") {
      const trimmedNote = deliveryNote.trim();
      const result = await approveDelivery({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          ...(trimmedNote.length > 0 ? { note: trimmedNote } : {}),
        },
      });
      setSubmitting(false);
      if (result._tag === "Failure") {
        setErrorMessage(
          workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
            gt("Could not approve delivery."),
        );
        return;
      }
      resetAndClose();
      props.onCompleted("ship");
      return;
    }

    const result = await returnForRevision({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        targetStage: activeReturnTarget,
        reason: reason.trim(),
      },
    });
    setSubmitting(false);
    if (result._tag === "Failure") {
      setErrorMessage(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not return card for revision."),
      );
      return;
    }
    resetAndClose();
    props.onCompleted(activeReturnTarget);
  };

  return (
    <AlertDialog
      open
      onOpenChange={(nextOpen) => {
        if (!nextOpen && !submitting) {
          resetAndClose();
        }
      }}
    >
      <AlertDialogPopup>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {transition.kind === "approve_spec" ? (
              <T>Approve spec and move to Plan</T>
            ) : transition.kind === "approve_plan" ? (
              <T>Approve plan and move to Execute</T>
            ) : transition.kind === "approve_delivery" ? (
              <T>Approve delivery and mark shipped</T>
            ) : (
              <T>Return card for revision</T>
            )}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {transition.kind === "approve_spec" ? (
              <T>
                Approve the current spec revision for <Var>{card.title}</Var> and advance the card
                to Plan.
              </T>
            ) : transition.kind === "approve_plan" ? (
              <T>
                Approve the current plan revision for <Var>{card.title}</Var> and advance the card
                to Execute.
              </T>
            ) : transition.kind === "approve_delivery" ? (
              <T>
                Approve delivery for <Var>{card.title}</Var> and create a durable delivery receipt
                without auto-committing changes.
              </T>
            ) : (
              <T>
                Returning <Var>{card.title}</Var> to{" "}
                <Var>{workflowBoardStageLabel(activeReturnTarget, messages)}</Var> preserves
                revision history and marks affected downstream approvals and evidence as stale.
              </T>
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="flex flex-col gap-3 px-6 pb-4">
          {errorMessage !== null ? (
            <Alert variant="error">
              <AlertDescription>
                <div className="flex flex-col gap-2">
                  <span>{errorMessage}</span>
                  {transition.kind === "approve_delivery" && props.onInspectShip !== undefined ? (
                    <div>
                      <Button
                        type="button"
                        size="xs"
                        variant="outline"
                        onClick={() => {
                          const targetCardId = card.cardId;
                          resetAndClose();
                          props.onInspectShip?.(targetCardId);
                        }}
                      >
                        <T>Inspect Ship blockers</T>
                      </Button>
                    </div>
                  ) : null}
                </div>
              </AlertDescription>
            </Alert>
          ) : null}

          {!permitted ? (
            <Alert variant="warning" role="note">
              <AlertDescription>
                <T>You do not have orchestration permission to perform this transition.</T>
              </AlertDescription>
            </Alert>
          ) : null}

          {transition.kind === "approve_delivery" && card.status === "blocked" ? (
            <Alert variant="warning" role="note">
              <AlertDescription>
                {card.blockerSummary ??
                  gt(
                    "Delivery is currently blocked by outstanding checks, findings, or acceptance criteria.",
                  )}
              </AlertDescription>
            </Alert>
          ) : null}

          {transition.kind === "approve_delivery" ? (
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="workflow-delivery-note"
                className="text-xs font-medium text-foreground"
              >
                <T>Delivery note (optional)</T>
              </label>
              <Textarea
                id="workflow-delivery-note"
                rows={2}
                placeholder={gt("Optional delivery note...")}
                value={deliveryNote}
                onChange={(event) => setDeliveryNote(event.target.value)}
                disabled={submitting}
              />
            </div>
          ) : null}

          {transition.kind === "return_for_revision" ? (
            <>
              {returnTargets.length > 1 ? (
                <div className="flex flex-col gap-1.5">
                  <span className="text-xs font-medium text-foreground">
                    <T>Target stage</T>
                  </span>
                  <Select
                    value={activeReturnTarget}
                    disabled={submitting}
                    onValueChange={(value) => {
                      if (value === "spec" || value === "plan" || value === "execute") {
                        setOverrideTarget(value);
                      }
                    }}
                  >
                    <SelectTrigger size="sm" aria-label={gt("Target stage")}>
                      <SelectValue>
                        {workflowBoardStageLabel(activeReturnTarget, messages)}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectPopup>
                      {returnTargets.map((stage) => (
                        <SelectItem key={stage} value={stage}>
                          {workflowBoardStageLabel(stage, messages)}
                        </SelectItem>
                      ))}
                    </SelectPopup>
                  </Select>
                </div>
              ) : null}

              <div className="flex flex-col gap-1.5">
                <label
                  htmlFor="workflow-return-reason"
                  className="text-xs font-medium text-foreground"
                >
                  <T>Reason for revision</T>
                </label>
                <Textarea
                  id="workflow-return-reason"
                  rows={3}
                  required
                  placeholder={gt("Describe what needs to be revised...")}
                  value={reason}
                  onChange={(event) => setReason(event.target.value)}
                  disabled={submitting}
                />
              </div>
            </>
          ) : null}
        </div>

        <AlertDialogFooter>
          <AlertDialogClose
            render={<Button type="button" variant="outline" disabled={submitting} />}
          >
            <T>Cancel</T>
          </AlertDialogClose>
          <Button type="button" disabled={!canConfirm} onClick={() => void handleConfirm()}>
            {submitting ? (
              <T>Applying…</T>
            ) : transition.kind === "approve_spec" ? (
              <T>Confirm spec approval</T>
            ) : transition.kind === "approve_plan" ? (
              <T>Confirm plan approval</T>
            ) : transition.kind === "approve_delivery" ? (
              <T>Confirm delivery approval</T>
            ) : (
              <T>Confirm return for revision</T>
            )}
          </Button>
        </AlertDialogFooter>
      </AlertDialogPopup>
    </AlertDialog>
  );
}

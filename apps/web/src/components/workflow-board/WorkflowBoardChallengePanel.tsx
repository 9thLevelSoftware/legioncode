import { useAtomValue } from "@effect/atom-react";
import { workflowBoardErrorMessage } from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  type EnvironmentId,
  type WorkflowBoardChallengeConcern,
  type WorkflowBoardChallengeConcernResponse,
  type WorkflowBoardChallengeDisposition,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { CheckCircle2Icon, ShieldAlertIcon } from "lucide-react";
import { useState } from "react";

import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import { isManualChallengeFallbackValid } from "./workflowBoard.logic";

interface ConcernDraft {
  readonly disposition: WorkflowBoardChallengeDisposition;
  readonly response: string;
  readonly reason: string;
}

export function WorkflowBoardChallengePanel(props: {
  readonly environmentId: EnvironmentId;
  readonly card: WorkflowCardDetail;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;

  const canChallenge = useAtomValue(
    workflowBoard.challengeStep.permissionAtom(props.environmentId),
  );
  const challengeStep = useAtomCommand(workflowBoard.challengeStep, {
    label: "workflow board challenge step",
    reportFailure: false,
  });

  const [concernDrafts, setConcernDrafts] = useState<Record<string, ConcernDraft>>({});
  const [manualOpen, setManualOpen] = useState(false);
  const [ackToken, setAckToken] = useState("");
  const [manualAnswers, setManualAnswers] = useState<readonly [string, string, string]>([
    "",
    "",
    "",
  ]);
  const [submitting, setSubmitting] = useState(false);

  const receipt = card.challengeReceipt ?? null;
  const concerns = receipt?.concerns ?? [];
  const synthesisCount = receipt?.synthesisCount ?? 0;
  const unresolvedConcernCount = concerns.filter((c) => c.disposition === "unresolved").length;

  const getDraft = (concern: WorkflowBoardChallengeConcern): ConcernDraft =>
    concernDrafts[concern.id] ?? {
      disposition: concern.disposition,
      response: concern.answer ?? "",
      reason: concern.reason ?? "",
    };

  const setDraftField = (concern: WorkflowBoardChallengeConcern, patch: Partial<ConcernDraft>) => {
    setConcernDrafts((prev) => ({
      ...prev,
      [concern.id]: {
        ...(prev[concern.id] ?? getDraft(concern)),
        ...patch,
      },
    }));
  };

  const dispositionLabel = (disp: WorkflowBoardChallengeDisposition): string => {
    switch (disp) {
      case "unresolved":
        return gt("Unresolved", { $context: "workflow board challenge disposition" });
      case "answer":
        return gt("Answer", { $context: "workflow board challenge disposition" });
      case "dismiss":
        return gt("Dismiss", { $context: "workflow board challenge disposition" });
      case "accept":
        return gt("Accept", { $context: "workflow board challenge disposition" });
      case "accept-risk":
        return gt("Accept risk", { $context: "workflow board challenge disposition" });
      case "defer":
        return gt("Defer", { $context: "workflow board challenge disposition" });
    }
  };

  const handleChallengeAction = async (
    action: "start" | "resolve" | "synthesize" | "complete" | "manualFallback",
  ) => {
    if (!canChallenge || submitting) return;
    props.onActionError(null);
    props.onActionNotice(null);

    if (action === "resolve") {
      const responses: WorkflowBoardChallengeConcernResponse[] = [];
      for (const concern of concerns) {
        const draft = getDraft(concern);
        if (draft.disposition === "answer" && draft.response.trim() === "") {
          props.onActionError(
            gt("Enter an answer for concern {concernId}.", { concernId: concern.id }),
          );
          return;
        }
        if (draft.disposition === "dismiss" && draft.reason.trim() === "") {
          props.onActionError(
            gt("Enter a dismissal reason for concern {concernId}.", { concernId: concern.id }),
          );
          return;
        }
        responses.push({
          concernId: concern.id,
          disposition: draft.disposition,
          ...(draft.response.trim() !== "" ? { response: draft.response.trim() } : {}),
          ...(draft.reason.trim() !== "" ? { reason: draft.reason.trim() } : {}),
        });
      }
      setSubmitting(true);
      const res = await challengeStep({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          action: "resolve",
          answers: responses,
        },
      });
      setSubmitting(false);
      if (res._tag === "Failure") {
        props.onActionError(
          workflowBoardErrorMessage(Cause.squash(res.cause), messages) ??
            gt("Could not save challenge responses."),
        );
        return;
      }
      setConcernDrafts({});
      props.onActionNotice(gt("Challenge dispositions saved."));
      return;
    }

    if (action === "manualFallback") {
      if (
        !isManualChallengeFallbackValid({
          acknowledgementToken: ackToken,
          answers: manualAnswers,
          unresolvedConcernCount,
        })
      ) {
        props.onActionError(
          gt(
            "Manual fallback requires resolving active concerns, 3 non-empty answers, and exact acknowledgement.",
          ),
        );
        return;
      }
      setSubmitting(true);
      const res = await challengeStep({
        environmentId: props.environmentId,
        input: {
          cardId: card.cardId,
          expectedRevision: PositiveInt.make(card.revision),
          action: "manualFallback",
          acknowledgementToken: "I acknowledge",
          unresolvedConcernIds: [],
          answers: [manualAnswers[0].trim(), manualAnswers[1].trim(), manualAnswers[2].trim()],
        },
      });
      setSubmitting(false);
      if (res._tag === "Failure") {
        props.onActionError(
          workflowBoardErrorMessage(Cause.squash(res.cause), messages) ??
            gt("Could not record manual challenge fallback."),
        );
        return;
      }
      setManualOpen(false);
      props.onActionNotice(gt("Manual 3-question challenge fallback recorded."));
      return;
    }

    setSubmitting(true);
    const res = await challengeStep({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        action,
      },
    });
    setSubmitting(false);
    if (res._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(res.cause), messages) ??
          gt("Challenge step failed."),
      );
      return;
    }
    props.onActionNotice(
      action === "synthesize"
        ? gt("Challenge synthesis complete while preserving verbatim human answers.")
        : action === "complete"
          ? gt("Challenge marked complete.")
          : gt("Challenge pass started."),
    );
  };

  const manualValid = isManualChallengeFallbackValid({
    acknowledgementToken: ackToken,
    answers: manualAnswers,
    unresolvedConcernCount,
  });

  return (
    <section
      aria-labelledby="challenge-section-heading"
      className="flex flex-col gap-3 rounded-lg border bg-card p-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 id="challenge-section-heading" className="text-sm font-semibold text-foreground">
            <T>Bounded challenge</T>
          </h3>
          {receipt !== null ? (
            <Badge
              variant={
                receipt.status === "stale"
                  ? "warning"
                  : receipt.status === "complete" ||
                      receipt.status === "resolved" ||
                      receipt.status === "synthesized" ||
                      receipt.status === "manual_fallback"
                    ? "success"
                    : "secondary"
              }
              size="sm"
            >
              {receipt.status}
            </Badge>
          ) : null}
          <Badge variant="outline" size="sm">
            <T>
              Concerns <Var>{concerns.length}</Var>/3
            </T>
          </Badge>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={!canChallenge || submitting}
            onClick={() => void handleChallengeAction("start")}
          >
            <ShieldAlertIcon />
            <T>Start challenge</T>
          </Button>
          {concerns.length > 0 ? (
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={!canChallenge || submitting || synthesisCount >= 1}
              onClick={() => void handleChallengeAction("synthesize")}
            >
              <T>
                Synthesize (<Var>{synthesisCount}</Var>/1)
              </T>
            </Button>
          ) : null}
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={!canChallenge || submitting || unresolvedConcernCount > 0}
            onClick={() => void handleChallengeAction("complete")}
          >
            <CheckCircle2Icon />
            <T>Complete challenge</T>
          </Button>
          <Button
            type="button"
            size="xs"
            variant="ghost"
            aria-expanded={manualOpen}
            onClick={() => setManualOpen((prev) => !prev)}
          >
            <T>Manual fallback</T>
          </Button>
        </div>
      </div>

      {receipt?.status === "stale" ? (
        <Alert variant="warning" role="note">
          <AlertDescription>
            <T>
              Challenge is stale because the spec or exploration answers changed. Re-run or complete
              challenge before approving the spec.
            </T>
          </AlertDescription>
        </Alert>
      ) : null}

      {concerns.length > 0 ? (
        <div className="flex flex-col gap-3">
          {concerns.map((concern) => {
            const draft = getDraft(concern);
            return (
              <div
                key={concern.id}
                className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-mono text-xs font-semibold text-foreground">
                    {concern.id}
                  </span>
                  <Select
                    value={draft.disposition}
                    disabled={!canChallenge || submitting}
                    onValueChange={(value) => {
                      if (
                        value === "unresolved" ||
                        value === "answer" ||
                        value === "dismiss" ||
                        value === "accept" ||
                        value === "accept-risk" ||
                        value === "defer"
                      ) {
                        setDraftField(concern, { disposition: value });
                      }
                    }}
                  >
                    <SelectTrigger
                      size="sm"
                      aria-label={gt("Disposition for {concernId}", { concernId: concern.id })}
                    >
                      <SelectValue>{dispositionLabel(draft.disposition)}</SelectValue>
                    </SelectTrigger>
                    <SelectPopup>
                      {(
                        [
                          "unresolved",
                          "answer",
                          "dismiss",
                          "accept",
                          "accept-risk",
                          "defer",
                        ] as const
                      ).map((disp) => (
                        <SelectItem key={disp} value={disp}>
                          {dispositionLabel(disp)}
                        </SelectItem>
                      ))}
                    </SelectPopup>
                  </Select>
                </div>

                <p className="text-xs font-medium text-foreground">{concern.summary}</p>
                {concern.whyMatters ? (
                  <p className="text-xs text-muted-foreground">
                    <T>
                      Why it matters: <Var>{concern.whyMatters}</Var>
                    </T>
                  </p>
                ) : null}
                {concern.evidence ? (
                  <p className="font-mono text-xs text-muted-foreground">
                    <T>
                      Evidence: <Var>{concern.evidence}</Var>
                    </T>
                  </p>
                ) : null}

                {draft.disposition === "answer" ? (
                  <Textarea
                    rows={2}
                    aria-label={gt("Answer for {concernId}", { concernId: concern.id })}
                    placeholder={gt("Provide an answer (preserved verbatim during synthesis)...")}
                    value={draft.response}
                    disabled={!canChallenge || submitting}
                    onChange={(event) => setDraftField(concern, { response: event.target.value })}
                  />
                ) : null}

                {draft.disposition === "dismiss" ? (
                  <Input
                    size="sm"
                    aria-label={gt("Dismissal reason for {concernId}", { concernId: concern.id })}
                    placeholder={gt("Why is this concern dismissed?")}
                    value={draft.reason}
                    disabled={!canChallenge || submitting}
                    onValueChange={(value) => setDraftField(concern, { reason: value })}
                  />
                ) : null}
              </div>
            );
          })}

          <div className="flex justify-end">
            <Button
              type="button"
              size="sm"
              disabled={!canChallenge || submitting}
              onClick={() => void handleChallengeAction("resolve")}
            >
              <T>Save challenge responses</T>
            </Button>
          </div>
        </div>
      ) : null}

      {receipt?.manualFallback ? (
        <div className="rounded-md border bg-muted/20 p-3 text-xs">
          <p className="font-medium text-foreground">
            <T>Manual fallback recorded</T>
          </p>
          <ul className="mt-1 list-disc space-y-0.5 ps-4 text-muted-foreground">
            <li key="q1">{receipt.manualFallback.answers[0]}</li>
            <li key="q2">{receipt.manualFallback.answers[1]}</li>
            <li key="q3">{receipt.manualFallback.answers[2]}</li>
          </ul>
        </div>
      ) : null}

      {manualOpen ? (
        <div className="flex flex-col gap-3 rounded-md border bg-muted/20 p-3">
          <p className="text-xs font-medium text-foreground">
            <T>Manual 3-question challenge fallback</T>
          </p>
          {unresolvedConcernCount > 0 ? (
            <Alert variant="warning" role="note">
              <AlertDescription>
                <T>Resolve or dispose all active concerns before using manual fallback.</T>
              </AlertDescription>
            </Alert>
          ) : null}

          <div className="flex flex-col gap-1.5">
            <label htmlFor="challenge-manual-q1" className="text-xs text-muted-foreground">
              <T>1. What failure mode or rollback boundary could invalidate this spec?</T>
            </label>
            <Input
              id="challenge-manual-q1"
              size="sm"
              value={manualAnswers[0]}
              disabled={!canChallenge || submitting}
              onValueChange={(val) => setManualAnswers([val, manualAnswers[1], manualAnswers[2]])}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="challenge-manual-q2" className="text-xs text-muted-foreground">
              <T>2. Which acceptance criteria or file contracts remain most exposed?</T>
            </label>
            <Input
              id="challenge-manual-q2"
              size="sm"
              value={manualAnswers[1]}
              disabled={!canChallenge || submitting}
              onValueChange={(val) => setManualAnswers([manualAnswers[0], val, manualAnswers[2]])}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="challenge-manual-q3" className="text-xs text-muted-foreground">
              <T>3. How will Plan verification confirm these assumptions?</T>
            </label>
            <Input
              id="challenge-manual-q3"
              size="sm"
              value={manualAnswers[2]}
              disabled={!canChallenge || submitting}
              onValueChange={(val) => setManualAnswers([manualAnswers[0], manualAnswers[1], val])}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="challenge-manual-ack" className="text-xs text-muted-foreground">
              <T>
                Type <code>I acknowledge</code> to confirm manual challenge completion
              </T>
            </label>
            <Input
              id="challenge-manual-ack"
              size="sm"
              value={ackToken}
              disabled={!canChallenge || submitting}
              onValueChange={setAckToken}
            />
          </div>
          <div className="flex justify-end">
            <Button
              type="button"
              size="sm"
              disabled={!canChallenge || submitting || !manualValid}
              onClick={() => void handleChallengeAction("manualFallback")}
            >
              <T>Submit manual fallback</T>
            </Button>
          </div>
        </div>
      ) : null}
    </section>
  );
}

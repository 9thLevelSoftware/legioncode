import { useAtomValue } from "@effect/atom-react";
import { workflowBoardErrorMessage } from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  type EnvironmentId,
  type WorkflowBoardExploreAction,
  type WorkflowBoardExploreAnswerInput,
  type WorkflowBoardExplorationDecision,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { PauseIcon, PlayIcon } from "lucide-react";
import { useState } from "react";

import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";

interface DecisionDraft {
  readonly action: Exclude<WorkflowBoardExploreAction, "pause" | "resume">;
  readonly answer: string;
}

export function WorkflowBoardExplorationWizard(props: {
  readonly environmentId: EnvironmentId;
  readonly card: WorkflowCardDetail;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;

  const canExplore = useAtomValue(workflowBoard.exploreStep.permissionAtom(props.environmentId));
  const exploreStep = useAtomCommand(workflowBoard.exploreStep, {
    label: "workflow board explore step",
    reportFailure: false,
  });

  const [drafts, setDrafts] = useState<Record<string, DecisionDraft>>({});
  const [submitting, setSubmitting] = useState(false);

  const exploration = card.exploration ?? null;
  const decisions = exploration?.decisions ?? [];
  const unresolved = decisions.filter((d) => d.status === "unresolved").slice(0, 3);
  const resolved = decisions.filter((d) => d.status === "resolved");
  const isPaused = exploration?.paused ?? false;
  const pendingRoundPrompt = exploration?.pendingRoundPrompt ?? false;

  const getDraft = (decision: WorkflowBoardExplorationDecision): DecisionDraft =>
    drafts[decision.id] ?? {
      action:
        decision.lastAction === "explain" ||
        decision.lastAction === "recommend" ||
        decision.lastAction === "edit" ||
        decision.lastAction === "unsure"
          ? decision.lastAction
          : "recommend",
      answer: decision.answer ?? "",
    };

  const setDecisionDraft = (
    decision: WorkflowBoardExplorationDecision,
    patch: Partial<DecisionDraft>,
  ) => {
    setDrafts((prev) => ({
      ...prev,
      [decision.id]: {
        ...(prev[decision.id] ?? getDraft(decision)),
        ...patch,
      },
    }));
  };

  const runExploreStep = async (payload: {
    readonly action?: "step" | "pause" | "resume";
    readonly continueRound?: boolean;
    readonly answers?: ReadonlyArray<WorkflowBoardExploreAnswerInput>;
  }) => {
    if (!canExplore || submitting) return;
    setSubmitting(true);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await exploreStep({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        ...(payload.action !== undefined ? { action: payload.action } : {}),
        ...(payload.continueRound !== undefined ? { continueRound: payload.continueRound } : {}),
        ...(payload.answers !== undefined && payload.answers.length > 0
          ? { answers: payload.answers }
          : {}),
      },
    });
    setSubmitting(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not update exploration."),
      );
      return;
    }

    setDrafts({});
    if (result.value.paused) {
      props.onActionNotice(
        gt("Exploration paused. Saved answers are preserved without approving the spec."),
      );
    } else if (result.value.pendingRoundPrompt) {
      props.onActionNotice(
        gt("Round complete. Continue to the next exploration round when ready."),
      );
    } else {
      props.onActionNotice(
        gt("Exploration updated (round {round}).", { round: result.value.roundIndex }),
      );
    }
  };

  const handleSubmitRoundAnswers = () => {
    const answers: WorkflowBoardExploreAnswerInput[] = [];
    for (const decision of unresolved) {
      const draft = getDraft(decision);
      if (draft.action === "edit" && draft.answer.trim() === "") {
        props.onActionError(
          gt("Enter an answer for decision {decisionId} before submitting.", {
            decisionId: decision.id,
          }),
        );
        return;
      }
      answers.push({
        decisionId: decision.id,
        action: draft.action,
        ...(draft.action === "edit" && draft.answer.trim() !== ""
          ? { answer: draft.answer.trim() }
          : {}),
      });
    }
    void runExploreStep({
      action: "step",
      ...(pendingRoundPrompt ? { continueRound: true } : {}),
      answers,
    });
  };

  const actionLabel = (action: DecisionDraft["action"]): string => {
    switch (action) {
      case "explain":
        return gt("Explain", { $context: "workflow board exploration decision action" });
      case "recommend":
        return gt("Recommend", { $context: "workflow board exploration decision action" });
      case "edit":
        return gt("Edit", { $context: "workflow board exploration decision action" });
      case "unsure":
        return gt("Unsure", { $context: "workflow board exploration decision action" });
    }
  };

  return (
    <section
      aria-labelledby="exploration-section-heading"
      className="flex flex-col gap-3 rounded-lg border bg-card p-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 id="exploration-section-heading" className="text-sm font-semibold text-foreground">
            <T>Guided exploration</T>
          </h3>
          {exploration !== null ? (
            <Badge variant="outline" size="sm">
              <T>
                Round <Var>{exploration.roundIndex}</Var>
              </T>
            </Badge>
          ) : null}
          {isPaused ? (
            <Badge variant="warning" size="sm">
              <T>Paused</T>
            </Badge>
          ) : null}
          {exploration !== null && exploration.unresolvedIds.length === 0 ? (
            <Badge variant="success" size="sm">
              <T>All decisions resolved</T>
            </Badge>
          ) : null}
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {exploration === null || decisions.length === 0 ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={!canExplore || submitting}
              onClick={() => void runExploreStep({ action: "step" })}
            >
              <PlayIcon />
              <T>Start exploration</T>
            </Button>
          ) : isPaused ? (
            <Button
              type="button"
              size="sm"
              disabled={!canExplore || submitting}
              onClick={() => void runExploreStep({ action: "resume" })}
            >
              <PlayIcon />
              <T>Resume exploration</T>
            </Button>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={!canExplore || submitting}
              onClick={() => void runExploreStep({ action: "pause" })}
            >
              <PauseIcon />
              <T>Pause</T>
            </Button>
          )}
        </div>
      </div>

      {unresolved.length > 0 ? (
        <div className="flex flex-col gap-3">
          {unresolved.map((decision) => {
            const draft = getDraft(decision);
            return (
              <div
                key={decision.id}
                className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-mono text-xs font-semibold text-foreground">
                    {decision.id}
                  </span>
                  <div className="flex flex-wrap items-center gap-1">
                    {(["explain", "recommend", "edit", "unsure"] as const).map((action) => (
                      <Button
                        key={action}
                        type="button"
                        size="xs"
                        variant={draft.action === action ? "secondary" : "ghost"}
                        aria-pressed={draft.action === action}
                        disabled={!canExplore || submitting}
                        onClick={() => setDecisionDraft(decision, { action })}
                      >
                        {actionLabel(action)}
                      </Button>
                    ))}
                  </div>
                </div>
                <p className="text-xs text-foreground">{decision.prompt}</p>
                {decision.explanation ? (
                  <p className="rounded bg-background p-2 text-xs text-muted-foreground">
                    {decision.explanation}
                  </p>
                ) : null}
                {decision.recommendation ? (
                  <p className="rounded bg-background p-2 text-xs text-muted-foreground">
                    <T>
                      Recommendation: <Var>{decision.recommendation}</Var>
                    </T>
                  </p>
                ) : null}
                {draft.action === "edit" ? (
                  <Input
                    size="sm"
                    aria-label={gt("Answer for {decisionId}", { decisionId: decision.id })}
                    placeholder={gt("Enter your decision answer...")}
                    value={draft.answer}
                    disabled={!canExplore || submitting}
                    onValueChange={(value) => setDecisionDraft(decision, { answer: value })}
                  />
                ) : null}
              </div>
            );
          })}

          <div className="flex flex-wrap items-center justify-end gap-2">
            {pendingRoundPrompt ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!canExplore || submitting}
                onClick={() => void runExploreStep({ action: "step", continueRound: true })}
              >
                <T>Continue next round</T>
              </Button>
            ) : null}
            <Button
              type="button"
              size="sm"
              disabled={!canExplore || submitting}
              onClick={handleSubmitRoundAnswers}
            >
              <T>Submit round answers</T>
            </Button>
          </div>
        </div>
      ) : decisions.length > 0 && pendingRoundPrompt && !isPaused ? (
        <div className="flex items-center justify-between gap-2 rounded-md border bg-muted/20 p-3">
          <span className="text-xs text-muted-foreground">
            <T>This exploration round is ready to advance.</T>
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={!canExplore || submitting}
            onClick={() => void runExploreStep({ action: "step", continueRound: true })}
          >
            <T>Continue next round</T>
          </Button>
        </div>
      ) : null}

      {resolved.length > 0 ? (
        <div className="flex flex-col gap-1.5 border-t pt-2">
          <span className="text-xs font-medium text-muted-foreground">
            <T>
              Resolved decisions (<Var>{resolved.length}</Var>)
            </T>
          </span>
          <ul className="space-y-1 text-xs">
            {resolved.map((decision) => (
              <li key={decision.id} className="rounded bg-muted/20 px-2.5 py-1.5">
                <span className="font-mono font-medium text-foreground">{decision.id}</span>
                {decision.answer ? (
                  <span className="text-muted-foreground"> : {decision.answer}</span>
                ) : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </section>
  );
}

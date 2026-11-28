import type { WorkflowBoardPlanAlternative } from "@legioncode/contracts";
import { T, useGT, Var } from "gt-react";
import { useId } from "react";

import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Textarea } from "../ui/textarea";

const DEFAULT_ALTERNATIVES: readonly [WorkflowBoardPlanAlternative, WorkflowBoardPlanAlternative] =
  [
    {
      id: "alt-1",
      title: "",
      summary: "",
      tradeoffs: "",
    },
    {
      id: "alt-2",
      title: "",
      summary: "",
      tradeoffs: "",
    },
  ];

export function WorkflowBoardPlanAlternatives(props: {
  readonly alternatives: ReadonlyArray<WorkflowBoardPlanAlternative>;
  readonly alternativesRevisionCount: number;
  readonly selectedAlternativeId: string | null;
  readonly selectionRationale: string;
  readonly reviseAlternativesFeedback: string;
  readonly disabled: boolean;
  readonly onAlternativesChange: (next: ReadonlyArray<WorkflowBoardPlanAlternative>) => void;
  readonly onSelectAlternative: (id: string | null) => void;
  readonly onRationaleChange: (value: string) => void;
  readonly onReviseFeedbackChange: (value: string) => void;
}) {
  const gt = useGT();
  const rationaleErrorId = useId();

  const pair: readonly [WorkflowBoardPlanAlternative, WorkflowBoardPlanAlternative] = [
    props.alternatives[0] ?? DEFAULT_ALTERNATIVES[0],
    props.alternatives[1] ?? DEFAULT_ALTERNATIVES[1],
  ];

  const updateAlternative = (index: 0 | 1, patch: Partial<WorkflowBoardPlanAlternative>) => {
    const prevAlt = pair[index];
    const nextAlt = { ...prevAlt, ...patch };
    const next: [WorkflowBoardPlanAlternative, WorkflowBoardPlanAlternative] = [
      index === 0 ? nextAlt : pair[0],
      index === 1 ? nextAlt : pair[1],
    ];
    props.onAlternativesChange(next);
    if (
      patch.id !== undefined &&
      props.selectedAlternativeId !== null &&
      prevAlt.id.trim() !== "" &&
      props.selectedAlternativeId.trim() === prevAlt.id.trim()
    ) {
      props.onSelectAlternative(patch.id.trim() === "" ? null : patch.id);
    }
  };

  const reviseLimitReached = props.alternativesRevisionCount >= 1;
  const rationaleMissing =
    props.selectedAlternativeId !== null && props.selectionRationale.trim() === "";

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-xs font-semibold uppercase text-muted-foreground">
          <T>Two-alternative comparison</T>
        </h4>
        <Badge variant="outline" size="sm">
          <T>
            Revisions <Var>{props.alternativesRevisionCount}</Var>/1
          </T>
        </Badge>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        {pair.map((alt, idx) => {
          const index = idx as 0 | 1;
          const selected =
            props.selectedAlternativeId !== null &&
            props.selectedAlternativeId.trim() === alt.id.trim();
          const canSelect =
            props.alternatives.length === 2 &&
            alt.id.trim() !== "" &&
            alt.title.trim() !== "" &&
            alt.summary.trim() !== "";

          return (
            <div
              key={index === 0 ? "alt-slot-0" : "alt-slot-1"}
              className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs font-semibold text-foreground">
                  <T>
                    Alternative <Var>{index + 1}</Var>
                  </T>
                </span>
                <Button
                  type="button"
                  size="xs"
                  variant={selected ? "default" : "outline"}
                  aria-pressed={selected}
                  disabled={props.disabled || (!selected && !canSelect)}
                  onClick={() => props.onSelectAlternative(selected ? null : alt.id)}
                >
                  {selected ? <T>Selected</T> : <T>Select alternative</T>}
                </Button>
              </div>

              <Input
                size="sm"
                font="mono"
                aria-label={gt("Alternative {number} ID", { number: index + 1 })}
                placeholder={gt("alt-1", { $context: "workflow board alternative id placeholder" })}
                value={alt.id}
                disabled={props.disabled}
                onValueChange={(value) => updateAlternative(index, { id: value })}
              />
              <Input
                size="sm"
                aria-label={gt("Alternative {number} title", { number: index + 1 })}
                placeholder={gt("Alternative title")}
                value={alt.title}
                disabled={props.disabled}
                onValueChange={(value) => updateAlternative(index, { title: value })}
              />
              <Textarea
                rows={2}
                aria-label={gt("Alternative {number} summary", { number: index + 1 })}
                placeholder={gt("Design approach summary")}
                value={alt.summary}
                disabled={props.disabled}
                onChange={(event) => updateAlternative(index, { summary: event.target.value })}
              />
              <Textarea
                rows={2}
                aria-label={gt("Alternative {number} tradeoffs", { number: index + 1 })}
                placeholder={gt("Key tradeoffs and risks")}
                value={alt.tradeoffs}
                disabled={props.disabled}
                onChange={(event) => updateAlternative(index, { tradeoffs: event.target.value })}
              />
            </div>
          );
        })}
      </div>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="plan-selection-rationale" className="text-xs font-medium text-foreground">
          <T>Selection rationale</T>
        </label>
        <Textarea
          id="plan-selection-rationale"
          rows={2}
          aria-invalid={rationaleMissing || undefined}
          aria-describedby={rationaleMissing ? rationaleErrorId : undefined}
          placeholder={gt("Explain why the chosen alternative best satisfies the spec...")}
          value={props.selectionRationale}
          disabled={props.disabled}
          onChange={(event) => props.onRationaleChange(event.target.value)}
        />
        {rationaleMissing ? (
          <p id={rationaleErrorId} role="alert" className="text-xs text-destructive-foreground">
            <T>Selection rationale is required when an alternative is selected.</T>
          </p>
        ) : null}
      </div>

      <div className="flex flex-col gap-1.5">
        <label
          htmlFor="plan-revise-alternatives"
          className="text-xs font-medium text-muted-foreground"
        >
          <T>Revise alternatives feedback (optional, max 1 use)</T>
        </label>
        <Input
          id="plan-revise-alternatives"
          size="sm"
          placeholder={
            reviseLimitReached
              ? gt("Alternative revision limit reached (1/1)")
              : gt("Request one revision to the two alternatives...")
          }
          value={props.reviseAlternativesFeedback}
          disabled={props.disabled || reviseLimitReached}
          onValueChange={props.onReviseFeedbackChange}
        />
      </div>
    </div>
  );
}

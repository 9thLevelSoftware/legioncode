import type { WorkflowBoardPlanLens, WorkflowBoardPlanLensEntry } from "@legioncode/contracts";
import { T, useGT } from "gt-react";

import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Textarea } from "../ui/textarea";
import { WORKFLOW_BOARD_PLAN_LENSES } from "./workflowBoard.logic";

export function WorkflowBoardPlanLenses(props: {
  readonly lenses: ReadonlyArray<WorkflowBoardPlanLensEntry>;
  readonly disabled: boolean;
  readonly onChange: (next: ReadonlyArray<WorkflowBoardPlanLensEntry>) => void;
}) {
  const gt = useGT();

  const lensLabel = (lens: WorkflowBoardPlanLens): string => {
    switch (lens) {
      case "ux":
        return gt("UX", { $context: "workflow board plan design lens" });
      case "functional":
        return gt("Functional", { $context: "workflow board plan design lens" });
      case "architecture":
        return gt("Architecture", { $context: "workflow board plan design lens" });
      case "quality":
        return gt("Quality", { $context: "workflow board plan design lens" });
      case "infrastructure":
        return gt("Infrastructure", { $context: "workflow board plan design lens" });
      case "delivery":
        return gt("Delivery", { $context: "workflow board plan design lens" });
    }
  };

  const entryFor = (lens: WorkflowBoardPlanLens): WorkflowBoardPlanLensEntry =>
    props.lenses.find((entry) => entry.lens === lens) ?? {
      lens,
      applicable: null,
      summary: null,
      reason: null,
    };

  const updateLens = (lens: WorkflowBoardPlanLens, nextEntry: WorkflowBoardPlanLensEntry) => {
    const map = new Map(props.lenses.map((item) => [item.lens, item]));
    map.set(lens, nextEntry);
    const ordered = WORKFLOW_BOARD_PLAN_LENSES.map(
      (candidate) =>
        map.get(candidate) ?? {
          lens: candidate,
          applicable: null,
          summary: null,
          reason: null,
        },
    );
    props.onChange(ordered);
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-2">
        <h4 className="text-xs font-semibold uppercase text-muted-foreground">
          <T>Six design lenses</T>
        </h4>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        {WORKFLOW_BOARD_PLAN_LENSES.map((lens) => {
          const entry = entryFor(lens);
          const label = lensLabel(lens);
          const missingText =
            (entry.applicable === true && (entry.summary?.trim() ?? "") === "") ||
            (entry.applicable === false && (entry.reason?.trim() ?? "") === "");
          const errorId = `plan-lens-error-${lens}`;

          return (
            <div key={lens} className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-xs font-semibold text-foreground">{label}</span>
                <div className="flex items-center gap-1">
                  <Button
                    type="button"
                    size="xs"
                    variant={entry.applicable === true ? "secondary" : "ghost"}
                    aria-pressed={entry.applicable === true}
                    disabled={props.disabled}
                    aria-label={gt("{lens} applicable", { lens: label })}
                    onClick={() =>
                      updateLens(lens, {
                        lens,
                        applicable: true,
                        summary: entry.summary ?? "",
                        reason: null,
                      })
                    }
                  >
                    <T>Applicable</T>
                  </Button>
                  <Button
                    type="button"
                    size="xs"
                    variant={entry.applicable === false ? "secondary" : "ghost"}
                    aria-pressed={entry.applicable === false}
                    disabled={props.disabled}
                    aria-label={gt("{lens} not applicable", { lens: label })}
                    onClick={() =>
                      updateLens(lens, {
                        lens,
                        applicable: false,
                        summary: null,
                        reason: entry.reason ?? "",
                      })
                    }
                  >
                    <T>Not applicable</T>
                  </Button>
                  {entry.applicable === null ? (
                    <Badge variant="outline" size="sm">
                      <T>Undecided</T>
                    </Badge>
                  ) : null}
                </div>
              </div>

              {entry.applicable === true ? (
                <Textarea
                  rows={2}
                  aria-label={gt("{lens} summary", { lens: label })}
                  aria-invalid={missingText || undefined}
                  aria-describedby={missingText ? errorId : undefined}
                  placeholder={gt("Describe the design decisions for this lens...")}
                  value={entry.summary ?? ""}
                  disabled={props.disabled}
                  onChange={(event) =>
                    updateLens(lens, {
                      ...entry,
                      summary: event.target.value,
                    })
                  }
                />
              ) : entry.applicable === false ? (
                <Input
                  size="sm"
                  aria-label={gt("{lens} non-applicable reason", { lens: label })}
                  aria-invalid={missingText || undefined}
                  aria-describedby={missingText ? errorId : undefined}
                  placeholder={gt("Why is this lens not applicable?")}
                  value={entry.reason ?? ""}
                  disabled={props.disabled}
                  onValueChange={(value) =>
                    updateLens(lens, {
                      ...entry,
                      reason: value,
                    })
                  }
                />
              ) : null}

              {missingText ? (
                <p id={errorId} role="alert" className="text-xs text-destructive-foreground">
                  {entry.applicable === true ? (
                    <T>Summary is required when applicable.</T>
                  ) : (
                    <T>Reason is required when not applicable.</T>
                  )}
                </p>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

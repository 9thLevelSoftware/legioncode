import type {
  WorkflowBoardAcEvidenceMapping,
  WorkflowBoardAcceptanceCriterion,
  WorkflowBoardEvidenceMethod,
  WorkflowBoardPlannedCheck,
} from "@legioncode/contracts";
import { T, useGT, Var } from "gt-react";
import { PlusIcon, Trash2Icon } from "lucide-react";

import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { formatArgvTokens, isSafeWorkflowBoardArgv, parseArgvTokens } from "./workflowBoard.logic";

export type DraftPlannedCheck = WorkflowBoardPlannedCheck & { readonly _rowKey?: string };

export function WorkflowBoardChecksAndAcMapper(props: {
  readonly checks: ReadonlyArray<DraftPlannedCheck>;
  readonly acceptanceCriteria: ReadonlyArray<WorkflowBoardAcceptanceCriterion>;
  readonly acMappings: ReadonlyArray<WorkflowBoardAcEvidenceMapping>;
  readonly disabled: boolean;
  readonly onChecksChange: (next: ReadonlyArray<DraftPlannedCheck>) => void;
  readonly onAcMappingsChange: (next: ReadonlyArray<WorkflowBoardAcEvidenceMapping>) => void;
}) {
  const gt = useGT();

  const methodLabel = (method: WorkflowBoardEvidenceMethod): string => {
    switch (method) {
      case "automated":
        return gt("Automated", { $context: "workflow board AC evidence method" });
      case "manual":
        return gt("Manual", { $context: "workflow board AC evidence method" });
      case "external":
        return gt("External", { $context: "workflow board AC evidence method" });
    }
  };

  const addCheck = () => {
    const nextId = `CHK-${String(props.checks.length + 1).padStart(2, "0")}`;
    props.onChecksChange([
      ...props.checks,
      {
        _rowKey: `chk-new-${props.checks.length}-${nextId}`,
        id: nextId,
        argv: ["vp", "test", "run"] as [string, ...string[]],
        description: "",
      },
    ]);
  };

  const updateCheck = (index: number, nextCheck: DraftPlannedCheck) => {
    const previous = props.checks[index];
    const stableKey =
      nextCheck._rowKey ?? previous?._rowKey ?? (previous?.id || `chk-row-${index}`);
    props.onChecksChange(
      props.checks.map((item, idx) =>
        idx === index ? { ...nextCheck, _rowKey: stableKey } : item,
      ),
    );
  };

  const removeCheck = (index: number) => {
    props.onChecksChange(props.checks.filter((_, idx) => idx !== index));
  };

  const setMappingForCriterion = (
    criterionId: string,
    nextMapping: WorkflowBoardAcEvidenceMapping,
  ) => {
    const existingIdx = props.acMappings.findIndex(
      (entry) => entry.criterionId.trim() === criterionId.trim(),
    );
    if (existingIdx >= 0) {
      props.onAcMappingsChange(
        props.acMappings.map((entry, idx) => (idx === existingIdx ? nextMapping : entry)),
      );
      return;
    }
    props.onAcMappingsChange([...props.acMappings, nextMapping]);
  };

  const mappedCount = props.acceptanceCriteria.filter((ac) =>
    props.acMappings.some((m) => m.criterionId.trim() === ac.id.trim()),
  ).length;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h4 className="text-xs font-semibold uppercase text-muted-foreground">
            <T>Argv-only planned checks</T>
          </h4>
          <Button
            type="button"
            size="xs"
            variant="outline"
            disabled={props.disabled}
            onClick={addCheck}
          >
            <PlusIcon />
            <T>Add check</T>
          </Button>
        </div>

        {props.checks.map((check, index) => {
          const idValid = check.id.trim().length > 0;
          const argvValid = isSafeWorkflowBoardArgv(check.argv);
          const checkRowKey = check._rowKey ?? check.id;
          return (
            <div
              key={checkRowKey}
              className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3"
            >
              <div className="grid gap-2 sm:grid-cols-4">
                <Input
                  size="sm"
                  font="mono"
                  aria-invalid={!idValid || undefined}
                  aria-describedby={!idValid ? `check-id-error-${index}` : undefined}
                  aria-label={gt("Check {number} ID", { number: index + 1 })}
                  value={check.id}
                  disabled={props.disabled}
                  onValueChange={(value) => updateCheck(index, { ...check, id: value })}
                />
                <div className="sm:col-span-2">
                  <Input
                    size="sm"
                    font="mono"
                    aria-invalid={!argvValid || undefined}
                    aria-describedby={!argvValid ? `check-argv-error-${index}` : undefined}
                    aria-label={gt("Check {number} argv command", { number: index + 1 })}
                    placeholder={gt("vp test run src/feature.test.ts", {
                      $context: "workflow board planned check argv placeholder",
                    })}
                    value={formatArgvTokens(check.argv)}
                    disabled={props.disabled}
                    onValueChange={(value) => {
                      const tokens = parseArgvTokens(value);
                      updateCheck(index, {
                        ...check,
                        argv: (tokens.length > 0 ? tokens : [""]) as [string, ...string[]],
                      });
                    }}
                  />
                </div>
                <div className="flex items-center justify-end gap-1">
                  <Button
                    type="button"
                    size="xs"
                    variant="ghost"
                    disabled={props.disabled}
                    aria-label={gt("Remove check {checkId}", {
                      checkId: check.id.trim() || String(index + 1),
                    })}
                    onClick={() => removeCheck(index)}
                  >
                    <Trash2Icon />
                    <T>Remove</T>
                  </Button>
                </div>
              </div>
              <Input
                size="sm"
                aria-label={gt("Check {number} description", { number: index + 1 })}
                placeholder={gt("What this check verifies")}
                value={check.description ?? ""}
                disabled={props.disabled}
                onValueChange={(value) => updateCheck(index, { ...check, description: value })}
              />
              {!idValid ? (
                <p
                  id={`check-id-error-${index}`}
                  role="alert"
                  className="text-xs text-destructive-foreground"
                >
                  <T>Check ID cannot be empty.</T>
                </p>
              ) : null}
              {!argvValid ? (
                <p
                  id={`check-argv-error-${index}`}
                  role="alert"
                  className="text-xs text-destructive-foreground"
                >
                  <T>
                    Check command must be an argv-only invocation (no shell wrappers or operators
                    like sh -c, cmd /c, &&, |, or $()).
                  </T>
                </p>
              ) : null}
            </div>
          );
        })}
      </div>

      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h4 className="text-xs font-semibold uppercase text-muted-foreground">
            <T>Acceptance criteria evidence mapping</T>
          </h4>
          <Badge
            variant={mappedCount === props.acceptanceCriteria.length ? "success" : "warning"}
            size="sm"
          >
            <T>
              Mapped <Var>{mappedCount}</Var>/<Var>{props.acceptanceCriteria.length}</Var>
            </T>
          </Badge>
        </div>

        {props.acceptanceCriteria.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            <T>No acceptance criteria defined in the spec yet.</T>
          </p>
        ) : (
          <div className="flex flex-col gap-2">
            {props.acceptanceCriteria.map((criterion) => {
              const existing = props.acMappings.find(
                (entry) => entry.criterionId.trim() === criterion.id.trim(),
              );
              const method: WorkflowBoardEvidenceMethod = existing?.method ?? "automated";

              return (
                <div
                  key={criterion.id}
                  className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="min-w-0 flex-1 text-xs">
                      <span className="font-mono font-semibold text-foreground">
                        {criterion.id}
                      </span>
                      <span className="text-muted-foreground"> : {criterion.text}</span>
                    </div>

                    <div className="flex items-center gap-2">
                      {existing === undefined ? (
                        <Badge variant="warning" size="sm">
                          <T>Unmapped</T>
                        </Badge>
                      ) : null}
                      <Select
                        value={method}
                        disabled={props.disabled}
                        onValueChange={(value) => {
                          if (value !== "automated" && value !== "manual" && value !== "external") {
                            return;
                          }
                          setMappingForCriterion(criterion.id, {
                            criterionId: criterion.id,
                            method: value,
                            ...(value === "automated"
                              ? {
                                  checkIds:
                                    props.checks[0]?.id !== undefined &&
                                    props.checks[0].id.trim() !== ""
                                      ? [props.checks[0].id]
                                      : [],
                                }
                              : {}),
                          });
                        }}
                      >
                        <SelectTrigger
                          size="sm"
                          aria-label={gt("Evidence method for {criterionId}", {
                            criterionId: criterion.id,
                          })}
                        >
                          <SelectValue>{methodLabel(method)}</SelectValue>
                        </SelectTrigger>
                        <SelectPopup>
                          <SelectItem value="automated">{methodLabel("automated")}</SelectItem>
                          <SelectItem value="manual">{methodLabel("manual")}</SelectItem>
                          <SelectItem value="external">{methodLabel("external")}</SelectItem>
                        </SelectPopup>
                      </Select>
                    </div>
                  </div>

                  {method === "automated" ? (
                    <Input
                      size="sm"
                      font="mono"
                      aria-label={gt("Check IDs for {criterionId}", { criterionId: criterion.id })}
                      placeholder={gt("CHK-01", {
                        $context: "workflow board planned check id placeholder",
                      })}
                      value={(existing?.checkIds ?? []).join(", ")}
                      disabled={props.disabled}
                      onValueChange={(value) =>
                        setMappingForCriterion(criterion.id, {
                          criterionId: criterion.id,
                          method: "automated",
                          checkIds: value
                            .split(",")
                            .map((item) => item.trim())
                            .filter((item) => item.length > 0),
                        })
                      }
                    />
                  ) : method === "manual" ? (
                    <Input
                      size="sm"
                      aria-label={gt("Manual verification procedure for {criterionId}", {
                        criterionId: criterion.id,
                      })}
                      placeholder={gt("Describe the manual verification steps...")}
                      value={existing?.procedure ?? ""}
                      disabled={props.disabled}
                      onValueChange={(value) =>
                        setMappingForCriterion(criterion.id, {
                          criterionId: criterion.id,
                          method: "manual",
                          ...(value.trim() !== "" ? { procedure: value } : {}),
                        })
                      }
                    />
                  ) : (
                    <div className="grid gap-2 sm:grid-cols-2">
                      <Input
                        size="sm"
                        aria-label={gt("External reference for {criterionId}", {
                          criterionId: criterion.id,
                        })}
                        placeholder={gt("External evidence reference or ticket")}
                        value={existing?.externalRef ?? ""}
                        disabled={props.disabled}
                        onValueChange={(value) =>
                          setMappingForCriterion(criterion.id, {
                            criterionId: criterion.id,
                            method: "external",
                            ...(value.trim() !== "" ? { externalRef: value } : {}),
                            ...(existing?.externalSource !== undefined &&
                            existing.externalSource.trim() !== ""
                              ? { externalSource: existing.externalSource }
                              : {}),
                          })
                        }
                      />
                      <Input
                        size="sm"
                        aria-label={gt("External source for {criterionId}", {
                          criterionId: criterion.id,
                        })}
                        placeholder={gt("External system or CI source")}
                        value={existing?.externalSource ?? ""}
                        disabled={props.disabled}
                        onValueChange={(value) =>
                          setMappingForCriterion(criterion.id, {
                            criterionId: criterion.id,
                            method: "external",
                            ...(value.trim() !== "" ? { externalSource: value } : {}),
                            ...(existing?.externalRef !== undefined &&
                            existing.externalRef.trim() !== ""
                              ? { externalRef: existing.externalRef }
                              : {}),
                          })
                        }
                      />
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

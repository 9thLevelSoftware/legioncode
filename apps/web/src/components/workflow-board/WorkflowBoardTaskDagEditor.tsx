import {
  WorkflowBoardNonGitArtifactKey,
  WorkflowBoardTaskContractsArray,
  type WorkflowBoardTaskContract,
  type WorkflowBoardTaskTestingMethod,
} from "@legioncode/contracts";
import * as Schema from "effect/Schema";
import { T, useGT, Var } from "gt-react";
import { PlusIcon, Trash2Icon } from "lucide-react";

import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  formatArgvTokens,
  isSafeWorkflowBoardArgv,
  isSafeWorkflowBoardNonGitPath,
  parseArgvTokens,
  parseSpecListLines,
} from "./workflowBoard.logic";

const isValidTaskContractsArray = Schema.is(WorkflowBoardTaskContractsArray);

function splitCommaOrLine(raw: string): string[] {
  return raw
    .split(/[,\r\n]+/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

export function WorkflowBoardTaskDagEditor(props: {
  readonly tasks: ReadonlyArray<WorkflowBoardTaskContract>;
  readonly disabled: boolean;
  readonly onChange: (next: ReadonlyArray<WorkflowBoardTaskContract>) => void;
}) {
  const gt = useGT();

  const testingMethodLabel = (method: WorkflowBoardTaskTestingMethod): string => {
    switch (method) {
      case "test-first":
        return gt("Test-first", { $context: "workflow board task testing method" });
      case "regression-first":
        return gt("Regression-first", { $context: "workflow board task testing method" });
      case "existing-checks":
        return gt("Existing checks", { $context: "workflow board task testing method" });
    }
  };

  const addTask = () => {
    const nextNumber = props.tasks.length + 1;
    const taskId = `TSK-${String(nextNumber).padStart(2, "0")}`;
    const newTask: WorkflowBoardTaskContract = {
      id: taskId,
      taskId,
      title: "",
      blockedBy: [],
      blocks: [],
      filesAllowed: [WorkflowBoardNonGitArtifactKey.make("src/feature.ts")],
      filesForbidden: [],
      verificationCommands: [],
      testingMethod: "test-first",
    };
    props.onChange([...props.tasks, newTask]);
  };

  const updateTask = (index: number, nextTask: WorkflowBoardTaskContract) => {
    const previous = props.tasks[index];
    const stableId = nextTask.id ?? previous?.id ?? (previous?.taskId || `task-row-${index}`);
    props.onChange(
      props.tasks.map((item, idx) => (idx === index ? { ...nextTask, id: stableId } : item)),
    );
  };

  const removeTask = (index: number) => {
    props.onChange(props.tasks.filter((_, idx) => idx !== index));
  };

  const dagValid = props.tasks.length === 0 || isValidTaskContractsArray(props.tasks);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-xs font-semibold uppercase text-muted-foreground">
          <T>Task DAG and exclusive file ownership</T>
        </h4>
        <Button
          type="button"
          size="xs"
          variant="outline"
          disabled={props.disabled}
          onClick={addTask}
        >
          <PlusIcon />
          <T>Add task</T>
        </Button>
      </div>

      {!dagValid ? (
        <p role="alert" className="text-xs text-destructive-foreground">
          <T>
            Task contracts must form an acyclic DAG with unique IDs, valid dependency references,
            and non-overlapping allowed files.
          </T>
        </p>
      ) : null}

      {props.tasks.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          <T>Add at least one task with exclusive file ownership before approving the plan.</T>
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          {props.tasks.map((task, index) => {
            const unsafeAllowed = task.filesAllowed.find(
              (path) => !isSafeWorkflowBoardNonGitPath(path),
            );
            const unsafeCommand = (task.verificationCommands ?? []).find(
              (argv) => !isSafeWorkflowBoardArgv(argv),
            );

            const rowKey = task.id ?? task.taskId;
            return (
              <div key={rowKey} className="flex flex-col gap-2 rounded-md border bg-muted/20 p-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="font-mono text-xs font-semibold text-foreground">
                    <T>
                      Task <Var>{index + 1}</Var>
                    </T>
                  </span>
                  <Button
                    type="button"
                    size="xs"
                    variant="ghost"
                    disabled={props.disabled}
                    aria-label={gt("Remove task {taskId}", {
                      taskId: task.taskId.trim() || String(index + 1),
                    })}
                    onClick={() => removeTask(index)}
                  >
                    <Trash2Icon />
                    <T>Remove</T>
                  </Button>
                </div>

                <div className="grid gap-2 sm:grid-cols-3">
                  <Input
                    size="sm"
                    font="mono"
                    aria-label={gt("Task {number} ID", { number: index + 1 })}
                    placeholder={gt("TSK-01", { $context: "workflow board task id placeholder" })}
                    value={task.taskId}
                    disabled={props.disabled}
                    onValueChange={(value) => updateTask(index, { ...task, taskId: value })}
                  />
                  <div className="sm:col-span-2">
                    <Input
                      size="sm"
                      aria-label={gt("Task {number} title", { number: index + 1 })}
                      placeholder={gt("Task summary")}
                      value={task.title}
                      disabled={props.disabled}
                      onValueChange={(value) => updateTask(index, { ...task, title: value })}
                    />
                  </div>
                </div>

                <div className="grid gap-2 sm:grid-cols-3">
                  <Input
                    size="sm"
                    font="mono"
                    aria-label={gt("Task {number} blocked by", { number: index + 1 })}
                    placeholder={gt("Blocked by (comma-separated IDs)")}
                    value={(task.blockedBy ?? []).join(", ")}
                    disabled={props.disabled}
                    onValueChange={(value) =>
                      updateTask(index, { ...task, blockedBy: splitCommaOrLine(value) })
                    }
                  />
                  <Input
                    size="sm"
                    font="mono"
                    aria-label={gt("Task {number} blocks", { number: index + 1 })}
                    placeholder={gt("Blocks (comma-separated IDs)")}
                    value={(task.blocks ?? []).join(", ")}
                    disabled={props.disabled}
                    onValueChange={(value) =>
                      updateTask(index, { ...task, blocks: splitCommaOrLine(value) })
                    }
                  />
                  <Select
                    value={task.testingMethod}
                    disabled={props.disabled}
                    onValueChange={(value) => {
                      if (
                        value === "test-first" ||
                        value === "regression-first" ||
                        value === "existing-checks"
                      ) {
                        updateTask(index, { ...task, testingMethod: value });
                      }
                    }}
                  >
                    <SelectTrigger
                      size="sm"
                      aria-label={gt("Task {number} testing method", { number: index + 1 })}
                    >
                      <SelectValue>{testingMethodLabel(task.testingMethod)}</SelectValue>
                    </SelectTrigger>
                    <SelectPopup>
                      <SelectItem value="test-first">{testingMethodLabel("test-first")}</SelectItem>
                      <SelectItem value="regression-first">
                        {testingMethodLabel("regression-first")}
                      </SelectItem>
                      <SelectItem value="existing-checks">
                        {testingMethodLabel("existing-checks")}
                      </SelectItem>
                    </SelectPopup>
                  </Select>
                </div>

                <div className="grid gap-2 sm:grid-cols-2">
                  <div className="flex flex-col gap-1">
                    <span className="text-xs text-muted-foreground">
                      <T>Allowed files (one per line)</T>
                    </span>
                    <Textarea
                      rows={2}
                      aria-label={gt("Task {number} allowed files", { number: index + 1 })}
                      aria-invalid={unsafeAllowed !== undefined || undefined}
                      aria-describedby={
                        unsafeAllowed !== undefined ? `task-allowed-error-${index}` : undefined
                      }
                      value={task.filesAllowed.join("\n")}
                      disabled={props.disabled}
                      onChange={(event) => {
                        const lines = parseSpecListLines(event.target.value);
                        updateTask(index, {
                          ...task,
                          filesAllowed: (lines.length > 0 ? lines : [""]) as [string, ...string[]],
                        });
                      }}
                    />
                  </div>
                  <div className="flex flex-col gap-1">
                    <span className="text-xs text-muted-foreground">
                      <T>Forbidden files (one per line)</T>
                    </span>
                    <Textarea
                      rows={2}
                      aria-label={gt("Task {number} forbidden files", { number: index + 1 })}
                      value={(task.filesForbidden ?? []).join("\n")}
                      disabled={props.disabled}
                      onChange={(event) =>
                        updateTask(index, {
                          ...task,
                          filesForbidden: parseSpecListLines(event.target.value),
                        })
                      }
                    />
                  </div>
                </div>

                <div className="flex flex-col gap-1">
                  <span className="text-xs text-muted-foreground">
                    <T>Task verification commands (argv tokens, one command per line)</T>
                  </span>
                  <Textarea
                    rows={2}
                    aria-label={gt("Task {number} verification commands", { number: index + 1 })}
                    aria-invalid={unsafeCommand !== undefined || undefined}
                    aria-describedby={
                      unsafeCommand !== undefined ? `task-cmd-error-${index}` : undefined
                    }
                    placeholder={gt("vp test run src/feature.test.ts", {
                      $context: "workflow board task verification command placeholder",
                    })}
                    value={(task.verificationCommands ?? [])
                      .map((argv) => formatArgvTokens(argv))
                      .join("\n")}
                    disabled={props.disabled}
                    onChange={(event) => {
                      const lines = parseSpecListLines(event.target.value);
                      updateTask(index, {
                        ...task,
                        verificationCommands: lines
                          .map((line) => parseArgvTokens(line))
                          .filter((tokens) => tokens.length > 0) as [string, ...string[]][],
                      });
                    }}
                  />
                </div>

                {unsafeAllowed !== undefined ? (
                  <p
                    id={`task-allowed-error-${index}`}
                    role="alert"
                    className="text-xs text-destructive-foreground"
                  >
                    <T>
                      Invalid allowed file path: <Var>{unsafeAllowed}</Var>
                    </T>
                  </p>
                ) : null}
                {unsafeCommand !== undefined ? (
                  <p
                    id={`task-cmd-error-${index}`}
                    role="alert"
                    className="text-xs text-destructive-foreground"
                  >
                    <T>Verification commands must be argv-only without shell operators.</T>
                  </p>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

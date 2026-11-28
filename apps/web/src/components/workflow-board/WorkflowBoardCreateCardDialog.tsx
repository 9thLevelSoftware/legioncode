import { useAtomValue } from "@effect/atom-react";
import { workflowBoardErrorMessage } from "@legioncode/client-runtime/workflow-board";
import {
  WorkflowBoardSafeInputPath,
  type EnvironmentId,
  type ModelSelection,
  type ProjectId,
  type ServerProvider,
  type ServerSettings,
  type WorkflowBoardAssistanceMode,
  type WorkflowBoardIntakeGoal,
  type WorkflowBoardSpecSourceType,
  type WorkflowCardId,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { useId, useState, type FormEvent } from "react";

import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription } from "../ui/alert";
import { Button } from "../ui/button";
import { Checkbox } from "../ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
} from "../ui/dialog";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  isSafeWorkflowBoardInputPath,
  isWorkflowBoardWorkspaceStrategyValid,
  normalizeWorkspaceStrategy,
  parseSpecListLines,
  resolveDefaultBoardLaunchConfig,
} from "./workflowBoard.logic";
import { WorkflowBoardLaunchControls } from "./WorkflowBoardLaunchControls";

export function WorkflowBoardCreateCardDialog(props: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly existingWorktreePaths: ReadonlyArray<string>;
  readonly onCreated: (cardId: WorkflowCardId) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const briefPathHintId = useId();
  const inputPathsHintId = useId();
  const wireframesCheckboxId = useId();
  const canCreate = useAtomValue(workflowBoard.createCard.permissionAtom(props.environmentId));
  const createCard = useAtomCommand(workflowBoard.createCard, {
    label: "workflow board create card",
    reportFailure: false,
  });

  const defaults = resolveDefaultBoardLaunchConfig({
    providers: props.providers,
    settings: props.settings,
    projectId: props.projectId,
  });

  const [title, setTitle] = useState("");
  const [sourceType, setSourceType] = useState<WorkflowBoardSpecSourceType>("idea");
  const [intakeGoal, setIntakeGoal] = useState<WorkflowBoardIntakeGoal>("change");
  const [brief, setBrief] = useState("");
  const [briefPath, setBriefPath] = useState("");
  const [inputPathsText, setInputPathsText] = useState("");
  const [assistanceMode, setAssistanceMode] = useState<WorkflowBoardAssistanceMode>("balanced");
  const [wireframesEnabled, setWireframesEnabled] = useState(false);
  const [modelSelection, setModelSelection] = useState<ModelSelection | null>(null);
  const [runtimeMode, setRuntimeMode] = useState(defaults.runtimeMode);
  const [interactionMode, setInteractionMode] = useState(defaults.interactionMode);
  const [workspaceStrategy, setWorkspaceStrategy] = useState(defaults.workspaceStrategy);
  const [submitting, setSubmitting] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const effectiveModelSelection = modelSelection ?? defaults.modelSelection;
  const workspaceStrategyValid = isWorkflowBoardWorkspaceStrategyValid(workspaceStrategy);

  const trimmedBriefPath = briefPath.trim();
  const briefPathInvalid =
    sourceType === "file" &&
    (trimmedBriefPath === "" || !isSafeWorkflowBoardInputPath(trimmedBriefPath));

  const parsedInputPaths = parseSpecListLines(inputPathsText);
  const invalidInputPath =
    parsedInputPaths.find((candidate) => !isSafeWorkflowBoardInputPath(candidate)) ?? null;

  const formValid =
    title.trim() !== "" && workspaceStrategyValid && !briefPathInvalid && invalidInputPath === null;

  const resetForm = () => {
    setTitle("");
    setSourceType("idea");
    setIntakeGoal("change");
    setBrief("");
    setBriefPath("");
    setInputPathsText("");
    setAssistanceMode("balanced");
    setWireframesEnabled(false);
    setModelSelection(null);
    setRuntimeMode(defaults.runtimeMode);
    setInteractionMode(defaults.interactionMode);
    setWorkspaceStrategy(defaults.workspaceStrategy);
    setErrorMessage(null);
  };

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();
    const trimmedTitle = title.trim();
    if (trimmedTitle === "" || submitting || !canCreate || !formValid) return;
    setSubmitting(true);
    setErrorMessage(null);

    const safeInputPaths = parsedInputPaths.map((entry) =>
      WorkflowBoardSafeInputPath.make(entry.trim()),
    );
    const trimmedBrief = brief.trim();

    const result = await createCard({
      environmentId: props.environmentId,
      input: {
        projectId: props.projectId,
        title: trimmedTitle,
        ...(trimmedBrief !== "" ? { brief } : {}),
        ...(sourceType === "file" && trimmedBriefPath !== ""
          ? { briefPath: WorkflowBoardSafeInputPath.make(trimmedBriefPath) }
          : {}),
        ...(safeInputPaths.length > 0 ? { inputPaths: safeInputPaths } : {}),
        intakeGoal,
        wireframesEnabled,
        ...(wireframesEnabled ? {} : { wireframesIndex: null }),
        sourceType,
        assistanceMode,
        workspaceStrategy: normalizeWorkspaceStrategy(workspaceStrategy),
        ...(effectiveModelSelection !== null ? { modelSelection: effectiveModelSelection } : {}),
        runtimeMode,
        interactionMode,
      },
    });
    setSubmitting(false);

    if (result._tag === "Failure") {
      setErrorMessage(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not create workflow card."),
      );
      return;
    }

    resetForm();
    props.onOpenChange(false);
    props.onCreated(result.value.cardId);
  };

  const assistanceLabel = (mode: WorkflowBoardAssistanceMode) => {
    switch (mode) {
      case "guided":
        return gt("Guided", { $context: "workflow board assistance mode" });
      case "balanced":
        return gt("Balanced", { $context: "workflow board assistance mode" });
      case "direct":
        return gt("Direct", { $context: "workflow board assistance mode" });
    }
  };

  const sourceTypeLabel = (mode: WorkflowBoardSpecSourceType) => {
    switch (mode) {
      case "idea":
        return gt("Start from idea", { $context: "workflow board spec source type" });
      case "file":
        return gt("Import brief file", { $context: "workflow board spec source type" });
      case "codebase":
        return gt("Investigate codebase", { $context: "workflow board spec source type" });
    }
  };

  const intakeGoalLabel = (goal: WorkflowBoardIntakeGoal) => {
    switch (goal) {
      case "greenfield":
        return gt("Greenfield", { $context: "workflow board intake goal" });
      case "change":
        return gt("Change", { $context: "workflow board intake goal" });
      case "audit":
        return gt("Audit", { $context: "workflow board intake goal" });
    }
  };

  return (
    <Dialog
      open={props.open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) setErrorMessage(null);
        props.onOpenChange(nextOpen);
      }}
    >
      <DialogPopup className="max-w-2xl">
        <form
          className="flex min-h-0 flex-1 flex-col"
          onSubmit={(event) => void handleSubmit(event)}
        >
          <DialogHeader>
            <DialogTitle>
              <T>New workflow card</T>
            </DialogTitle>
            <DialogDescription>
              <T>
                Create a feature card starting in Spec from an idea, local brief, or codebase
                investigation.
              </T>
            </DialogDescription>
          </DialogHeader>
          <DialogPanel>
            {errorMessage !== null ? (
              <Alert variant="error">
                <AlertDescription>{errorMessage}</AlertDescription>
              </Alert>
            ) : null}

            <div className="flex flex-col gap-1.5">
              <label htmlFor="workflow-card-title" className="text-xs font-medium text-foreground">
                <T>Title</T>
              </label>
              <Input
                id="workflow-card-title"
                required
                placeholder={gt("Feature or spec title")}
                value={title}
                onValueChange={setTitle}
                disabled={submitting}
              />
            </div>

            <div className="grid gap-3 sm:grid-cols-3">
              <div className="flex flex-col gap-1.5">
                <span className="text-xs font-medium text-foreground">
                  <T>Source</T>
                </span>
                <Select
                  value={sourceType}
                  disabled={submitting}
                  onValueChange={(value) => {
                    if (value === "idea" || value === "file" || value === "codebase") {
                      setSourceType(value);
                    }
                  }}
                >
                  <SelectTrigger size="sm" aria-label={gt("Source")}>
                    <SelectValue>{sourceTypeLabel(sourceType)}</SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    <SelectItem value="idea">{sourceTypeLabel("idea")}</SelectItem>
                    <SelectItem value="file">{sourceTypeLabel("file")}</SelectItem>
                    <SelectItem value="codebase">{sourceTypeLabel("codebase")}</SelectItem>
                  </SelectPopup>
                </Select>
              </div>

              <div className="flex flex-col gap-1.5">
                <span className="text-xs font-medium text-foreground">
                  <T>Intake goal</T>
                </span>
                <Select
                  value={intakeGoal}
                  disabled={submitting}
                  onValueChange={(value) => {
                    if (value === "greenfield" || value === "change" || value === "audit") {
                      setIntakeGoal(value);
                    }
                  }}
                >
                  <SelectTrigger size="sm" aria-label={gt("Intake goal")}>
                    <SelectValue>{intakeGoalLabel(intakeGoal)}</SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    <SelectItem value="greenfield">{intakeGoalLabel("greenfield")}</SelectItem>
                    <SelectItem value="change">{intakeGoalLabel("change")}</SelectItem>
                    <SelectItem value="audit">{intakeGoalLabel("audit")}</SelectItem>
                  </SelectPopup>
                </Select>
              </div>

              <div className="flex flex-col gap-1.5">
                <span className="text-xs font-medium text-foreground">
                  <T>Assistance mode</T>
                </span>
                <Select
                  value={assistanceMode}
                  disabled={submitting}
                  onValueChange={(value) => {
                    if (value === "guided" || value === "balanced" || value === "direct") {
                      setAssistanceMode(value);
                    }
                  }}
                >
                  <SelectTrigger size="sm" aria-label={gt("Assistance mode")}>
                    <SelectValue>{assistanceLabel(assistanceMode)}</SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    <SelectItem value="guided">{assistanceLabel("guided")}</SelectItem>
                    <SelectItem value="balanced">{assistanceLabel("balanced")}</SelectItem>
                    <SelectItem value="direct">{assistanceLabel("direct")}</SelectItem>
                  </SelectPopup>
                </Select>
              </div>
            </div>

            {sourceType === "file" ? (
              <div className="flex flex-col gap-1.5">
                <label
                  htmlFor="workflow-card-brief-path"
                  className="text-xs font-medium text-foreground"
                >
                  <T>Brief file path (relative to workspace)</T>
                </label>
                <Input
                  id="workflow-card-brief-path"
                  font="mono"
                  required
                  aria-invalid={briefPathInvalid || undefined}
                  aria-describedby={briefPathInvalid ? briefPathHintId : undefined}
                  placeholder={gt("docs/brief.md", {
                    $context: "workflow board relative markdown path placeholder",
                  })}
                  value={briefPath}
                  onValueChange={setBriefPath}
                  disabled={submitting}
                />
                {briefPathInvalid ? (
                  <p
                    id={briefPathHintId}
                    role="alert"
                    className="text-xs text-destructive-foreground"
                  >
                    <T>
                      Enter a safe workspace-relative path (no traversal, .git, or credential
                      files).
                    </T>
                  </p>
                ) : null}
              </div>
            ) : null}

            <div className="flex flex-col gap-1.5">
              <label htmlFor="workflow-card-brief" className="text-xs font-medium text-foreground">
                <T>Initial spec or idea</T>
              </label>
              <Textarea
                id="workflow-card-brief"
                rows={4}
                placeholder={gt("Describe the problem, goals, and constraints...")}
                value={brief}
                onChange={(event) => setBrief(event.target.value)}
                disabled={submitting}
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="workflow-card-input-paths"
                className="text-xs font-medium text-foreground"
              >
                <T>Input reference files (optional, one per line)</T>
              </label>
              <Textarea
                id="workflow-card-input-paths"
                rows={2}
                aria-invalid={invalidInputPath !== null || undefined}
                aria-describedby={invalidInputPath !== null ? inputPathsHintId : undefined}
                placeholder={gt("README.md", {
                  $context: "workflow board relative input file placeholder",
                })}
                value={inputPathsText}
                onChange={(event) => setInputPathsText(event.target.value)}
                disabled={submitting}
              />
              {invalidInputPath !== null ? (
                <p
                  id={inputPathsHintId}
                  role="alert"
                  className="text-xs text-destructive-foreground"
                >
                  <T>
                    Invalid workspace-relative path: <Var>{invalidInputPath}</Var>
                  </T>
                </p>
              ) : null}
            </div>

            <label
              htmlFor={wireframesCheckboxId}
              className="flex cursor-pointer items-center gap-2 text-xs text-foreground"
            >
              <Checkbox
                id={wireframesCheckboxId}
                checked={wireframesEnabled}
                onCheckedChange={(checked) => setWireframesEnabled(Boolean(checked))}
                disabled={submitting}
              />
              <span>
                <T>Enable optional HTML wireframes (leave unchecked for non-UI features)</T>
              </span>
            </label>

            <WorkflowBoardLaunchControls
              environmentId={props.environmentId}
              projectId={props.projectId}
              providers={props.providers}
              settings={props.settings}
              existingWorktreePaths={props.existingWorktreePaths}
              modelSelection={effectiveModelSelection}
              onModelSelectionChange={setModelSelection}
              runtimeMode={runtimeMode}
              onRuntimeModeChange={setRuntimeMode}
              interactionMode={interactionMode}
              onInteractionModeChange={setInteractionMode}
              workspaceStrategy={workspaceStrategy}
              onWorkspaceStrategyChange={setWorkspaceStrategy}
              disabled={submitting}
            />
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button type="button" variant="outline" />}>
              <T>Cancel</T>
            </DialogClose>
            <Button type="submit" disabled={!canCreate || submitting || !formValid}>
              {submitting ? <T>Creating…</T> : <T>Create card</T>}
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}

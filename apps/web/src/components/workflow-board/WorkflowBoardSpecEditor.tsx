import { useAtomValue } from "@effect/atom-react";
import { workflowBoardErrorMessage } from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  WorkflowBoardSafeInputPath,
  type EnvironmentId,
  type ModelSelection,
  type ProviderInteractionMode,
  type RuntimeMode,
  type ServerProvider,
  type ServerSettings,
  type ThreadId,
  type WorkflowBoardIntakeGoal,
  type WorkflowBoardSpecDiff,
  type WorkflowBoardSpecSourceType,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { PlayIcon, SaveIcon } from "lucide-react";
import { useId, useState } from "react";

import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "../ui/collapsible";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  formatAcceptanceCriteriaLines,
  isSafeWorkflowBoardInputPath,
  normalizeWorkspaceStrategy,
  parseAcceptanceCriteriaLines,
  parseSpecListLines,
  resolveDefaultBoardLaunchConfig,
} from "./workflowBoard.logic";

interface SpecDraftState {
  readonly baseRevision: number;
  readonly title: string;
  readonly brief: string;
  readonly intakeGoal: WorkflowBoardIntakeGoal;
  readonly inputPathsText: string;
  readonly acceptanceText: string;
  readonly constraintsText: string;
  readonly sourceRefsText: string;
}

function buildSpecDraft(card: WorkflowCardDetail): SpecDraftState {
  return {
    baseRevision: card.revision,
    title: card.spec.title,
    brief: card.spec.brief,
    intakeGoal: card.spec.intakeGoal ?? "change",
    inputPathsText: (card.spec.sourceInputDigests ?? []).map((entry) => entry.path).join("\n"),
    acceptanceText: formatAcceptanceCriteriaLines(card.spec.acceptanceCriteria),
    constraintsText: card.spec.constraints.join("\n"),
    sourceRefsText: card.spec.sourceRefs.join("\n"),
  };
}

export function WorkflowBoardSpecEditor(props: {
  readonly environmentId: EnvironmentId;
  readonly card: WorkflowCardDetail;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly modelSelection: ModelSelection | null;
  readonly runtimeMode: RuntimeMode;
  readonly interactionMode: ProviderInteractionMode;
  readonly workspaceStrategy: WorkflowCardDetail["workspaceStrategy"];
  readonly workspaceStrategyValid: boolean;
  readonly activeSpecThreadId: ThreadId | null;
  readonly onSpecStarted?: (threadId: ThreadId) => void;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const inputPathsHintId = useId();
  const { card } = props;

  const canUpdateSpec = useAtomValue(workflowBoard.updateSpec.permissionAtom(props.environmentId));
  const canStartStage = useAtomValue(workflowBoard.startStage.permissionAtom(props.environmentId));

  const updateSpec = useAtomCommand(workflowBoard.updateSpec, {
    label: "workflow board update spec",
    reportFailure: false,
  });
  const startStage = useAtomCommand(workflowBoard.startStage, {
    label: "workflow board start stage",
    reportFailure: false,
  });

  const [draft, setDraft] = useState<SpecDraftState | null>(null);
  const [savingSpec, setSavingSpec] = useState(false);
  const [startingSpec, setStartingSpec] = useState(false);
  const [lastDiff, setLastDiff] = useState<WorkflowBoardSpecDiff | null>(null);

  const activeDraft = draft ?? buildSpecDraft(card);
  const draftStaleVsServer = draft !== null && draft.baseRevision !== card.revision;

  const updateDraftField = (patch: Partial<SpecDraftState>) => {
    setDraft((prev) => ({
      ...(prev ?? buildSpecDraft(card)),
      ...patch,
    }));
  };

  const parsedInputPaths = parseSpecListLines(activeDraft.inputPathsText);
  const invalidInputPath =
    parsedInputPaths.find((candidate) => !isSafeWorkflowBoardInputPath(candidate)) ?? null;

  const launchDefaults = resolveDefaultBoardLaunchConfig({
    providers: props.providers,
    settings: props.settings,
    projectId: card.projectId,
    card,
  });
  const effectiveModelSelection = props.modelSelection ?? launchDefaults.modelSelection;

  const sourceTypeLabel = (source: WorkflowBoardSpecSourceType): string => {
    switch (source) {
      case "idea":
        return gt("Idea", { $context: "workflow board spec source type badge" });
      case "file":
        return gt("Brief file", { $context: "workflow board spec source type badge" });
      case "codebase":
        return gt("Codebase", { $context: "workflow board spec source type badge" });
    }
  };

  const intakeGoalLabel = (goal: WorkflowBoardIntakeGoal): string => {
    switch (goal) {
      case "greenfield":
        return gt("Greenfield", { $context: "workflow board intake goal" });
      case "change":
        return gt("Change", { $context: "workflow board intake goal" });
      case "audit":
        return gt("Audit", { $context: "workflow board intake goal" });
    }
  };

  const handleSaveSpec = async () => {
    const trimmedTitle = activeDraft.title.trim();
    if (trimmedTitle === "" || savingSpec || !canUpdateSpec || invalidInputPath !== null) return;
    setSavingSpec(true);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await updateSpec({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        title: trimmedTitle,
        brief: activeDraft.brief,
        intakeGoal: activeDraft.intakeGoal,
        inputPaths: parsedInputPaths.map((entry) => WorkflowBoardSafeInputPath.make(entry.trim())),
        acceptanceCriteria: parseAcceptanceCriteriaLines(activeDraft.acceptanceText),
        constraints: parseSpecListLines(activeDraft.constraintsText),
        sourceRefs: parseSpecListLines(activeDraft.sourceRefsText),
      },
    });
    setSavingSpec(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not save spec."),
      );
      return;
    }

    setDraft(null);
    setLastDiff(result.value.diff ?? null);
    props.onActionNotice(
      gt("Spec saved at revision {revision}.", { revision: result.value.revision }),
    );
  };

  const handleStartSpecThread = async () => {
    if (
      !canStartStage ||
      !props.settings.workflowsEnabled ||
      startingSpec ||
      effectiveModelSelection === null ||
      !props.workspaceStrategyValid
    ) {
      return;
    }
    setStartingSpec(true);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await startStage({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stage: "spec",
        modelSelection: effectiveModelSelection,
        runtimeMode: props.runtimeMode,
        interactionMode: props.interactionMode,
        workspaceStrategy: normalizeWorkspaceStrategy(props.workspaceStrategy),
      },
    });
    setStartingSpec(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not start Spec thread."),
      );
      return;
    }
    props.onSpecStarted?.(result.value.threadId);
    props.onActionNotice(
      result.value.resumed
        ? gt("Resumed Spec authoring thread.")
        : gt("Started Spec authoring thread."),
    );
  };

  const intake = card.spec.intakeAnalysis ?? null;

  return (
    <section aria-labelledby="spec-section-heading" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 id="spec-section-heading" className="text-sm font-semibold text-foreground">
            <T>Spec authoring</T>
          </h3>
          <Badge variant="outline" size="sm">
            {sourceTypeLabel(card.spec.sourceType)}
          </Badge>
          <Badge variant="secondary" size="sm">
            {card.spec.assistanceMode}
          </Badge>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={
              !canStartStage ||
              !props.settings.workflowsEnabled ||
              startingSpec ||
              effectiveModelSelection === null ||
              !props.workspaceStrategyValid
            }
            onClick={() => void handleStartSpecThread()}
          >
            <PlayIcon />
            {startingSpec ? (
              <T>Starting Spec…</T>
            ) : props.activeSpecThreadId !== null ? (
              <T>Resume Spec thread</T>
            ) : (
              <T>Start Spec thread</T>
            )}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={
              !canUpdateSpec ||
              savingSpec ||
              activeDraft.title.trim() === "" ||
              invalidInputPath !== null
            }
            onClick={() => void handleSaveSpec()}
          >
            <SaveIcon />
            {savingSpec ? <T>Saving…</T> : <T>Save spec</T>}
          </Button>
        </div>
      </div>

      {draftStaleVsServer ? (
        <Alert variant="warning" role="note">
          <AlertTitle>
            <T>Newer server revision available</T>
          </AlertTitle>
          <AlertDescription>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span>
                <T>
                  Your local draft started at revision <Var>{draft.baseRevision}</Var>, while the
                  server is at revision <Var>{card.revision}</Var>.
                </T>
              </span>
              <Button type="button" size="xs" variant="outline" onClick={() => setDraft(null)}>
                <T>Reset to server</T>
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      ) : null}

      {intake !== null ? (
        <div className="rounded-lg border bg-muted/20 p-3 text-xs">
          <p className="font-medium text-foreground">
            <T>Brownfield intake scan</T>
          </p>
          <div className="mt-2 grid gap-2 sm:grid-cols-3">
            <div>
              <span className="font-medium text-muted-foreground">
                <T>
                  Supplied decisions (<Var>{intake.suppliedDecisions.length}</Var>)
                </T>
              </span>
              <ul className="mt-1 list-disc space-y-0.5 ps-4 text-muted-foreground">
                {intake.suppliedDecisions.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </div>
            <div>
              <span className="font-medium text-muted-foreground">
                <T>
                  Inferred codebase signals (<Var>{intake.inferredDecisions.length}</Var>)
                </T>
              </span>
              <ul className="mt-1 list-disc space-y-0.5 ps-4 text-muted-foreground">
                {intake.inferredDecisions.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </div>
            <div>
              <span className="font-medium text-muted-foreground">
                <T>
                  Missing decisions (<Var>{intake.missingDecisions.length}</Var>)
                </T>
              </span>
              <ul className="mt-1 list-disc space-y-0.5 ps-4 text-muted-foreground">
                {intake.missingDecisions.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </div>
          </div>
        </div>
      ) : null}

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1.5">
          <label htmlFor="spec-title-input" className="text-xs font-medium text-muted-foreground">
            <T>Spec title</T>
          </label>
          <Input
            id="spec-title-input"
            value={activeDraft.title}
            onValueChange={(value) => updateDraftField({ title: value })}
            disabled={savingSpec || !canUpdateSpec}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-medium text-muted-foreground">
            <T>Intake goal</T>
          </span>
          <Select
            value={activeDraft.intakeGoal}
            disabled={savingSpec || !canUpdateSpec}
            onValueChange={(value) => {
              if (value === "greenfield" || value === "change" || value === "audit") {
                updateDraftField({ intakeGoal: value });
              }
            }}
          >
            <SelectTrigger size="sm" aria-label={gt("Intake goal")}>
              <SelectValue>{intakeGoalLabel(activeDraft.intakeGoal)}</SelectValue>
            </SelectTrigger>
            <SelectPopup>
              <SelectItem value="greenfield">{intakeGoalLabel("greenfield")}</SelectItem>
              <SelectItem value="change">{intakeGoalLabel("change")}</SelectItem>
              <SelectItem value="audit">{intakeGoalLabel("audit")}</SelectItem>
            </SelectPopup>
          </Select>
        </div>
      </div>

      <div className="flex flex-col gap-1.5">
        <label htmlFor="spec-brief-input" className="text-xs font-medium text-muted-foreground">
          <T>Spec brief (Markdown)</T>
        </label>
        <Textarea
          id="spec-brief-input"
          rows={6}
          value={activeDraft.brief}
          onChange={(event) => updateDraftField({ brief: event.target.value })}
          disabled={savingSpec || !canUpdateSpec}
        />
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1.5">
          <label htmlFor="spec-ac-input" className="text-xs font-medium text-muted-foreground">
            <T>Acceptance criteria (one per line)</T>
          </label>
          <Textarea
            id="spec-ac-input"
            rows={3}
            placeholder={gt("AC-1: Board displays four stage columns")}
            value={activeDraft.acceptanceText}
            onChange={(event) => updateDraftField({ acceptanceText: event.target.value })}
            disabled={savingSpec || !canUpdateSpec}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="spec-constraints-input"
            className="text-xs font-medium text-muted-foreground"
          >
            <T>Constraints (one per line)</T>
          </label>
          <Textarea
            id="spec-constraints-input"
            rows={3}
            value={activeDraft.constraintsText}
            onChange={(event) => updateDraftField({ constraintsText: event.target.value })}
            disabled={savingSpec || !canUpdateSpec}
          />
        </div>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1.5">
          <label htmlFor="spec-refs-input" className="text-xs font-medium text-muted-foreground">
            <T>Source references (one per line)</T>
          </label>
          <Textarea
            id="spec-refs-input"
            rows={2}
            value={activeDraft.sourceRefsText}
            onChange={(event) => updateDraftField({ sourceRefsText: event.target.value })}
            disabled={savingSpec || !canUpdateSpec}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label
            htmlFor="spec-input-paths-input"
            className="text-xs font-medium text-muted-foreground"
          >
            <T>Input file paths (one per line)</T>
          </label>
          <Textarea
            id="spec-input-paths-input"
            rows={2}
            aria-invalid={invalidInputPath !== null || undefined}
            aria-describedby={invalidInputPath !== null ? inputPathsHintId : undefined}
            value={activeDraft.inputPathsText}
            onChange={(event) => updateDraftField({ inputPathsText: event.target.value })}
            disabled={savingSpec || !canUpdateSpec}
          />
          {invalidInputPath !== null ? (
            <p id={inputPathsHintId} role="alert" className="text-xs text-destructive-foreground">
              <T>
                Invalid workspace-relative path: <Var>{invalidInputPath}</Var>
              </T>
            </p>
          ) : null}
        </div>
      </div>

      {lastDiff !== null && lastDiff.unifiedDiff.trim() !== "" ? (
        <Collapsible>
          <CollapsibleTrigger className="rounded-sm text-xs font-medium text-primary underline underline-offset-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
            <T>
              View latest spec diff (<Var>{lastDiff.changedFields.join(", ")}</Var>)
            </T>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <pre className="mt-2 max-h-60 overflow-auto rounded-md border bg-muted/30 p-3 font-mono text-xs text-foreground">
              {lastDiff.unifiedDiff}
            </pre>
          </CollapsibleContent>
        </Collapsible>
      ) : null}
    </section>
  );
}

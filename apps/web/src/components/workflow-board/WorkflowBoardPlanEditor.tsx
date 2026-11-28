import { useAtomValue } from "@effect/atom-react";
import {
  workflowBoardErrorMessage,
  workflowBoardLatestStageRun,
} from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  WorkflowBoardAcEvidenceMappingsArray,
  WorkflowBoardArtifactKey,
  WorkflowBoardPlanAlternativesPair,
  WorkflowBoardPlanLensesArray,
  WorkflowBoardTaskContractsArray,
  type EnvironmentId,
  type ModelSelection,
  type ProviderInteractionMode,
  type RuntimeMode,
  type ServerProvider,
  type ServerSettings,
  type ThreadId,
  type WorkflowBoardAcEvidenceMapping,
  type WorkflowBoardPlanAlternative,
  type WorkflowBoardPlanLensEntry,
  type WorkflowBoardPlanningStrategy,
  type WorkflowBoardStrategyMetadata,
  type WorkflowBoardTaskContract,
  type WorkflowBoardTaskGranularity,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import * as Schema from "effect/Schema";
import { T, useGT, useMessages, Var } from "gt-react";
import { CheckCircle2Icon, PlayIcon, SaveIcon } from "lucide-react";
import { useState } from "react";

import { useEnvironmentQuery } from "../../state/query";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import ChatMarkdown from "../ChatMarkdown";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { Textarea } from "../ui/textarea";
import {
  WORKFLOW_BOARD_PLAN_LENSES,
  canApproveWorkflowBoardPlan,
  canStartWorkflowBoardPlan,
  collectPlanApprovalBlockers,
  isPlanOnlyEditPreservingSpecApproval,
  isPlanStrategyValid,
  isSafeWorkflowBoardArgv,
  isWorkflowBoardWorkspaceStrategyValid,
  normalizeAcEvidenceMappings,
  normalizeWorkspaceStrategy,
  resolveDefaultBoardLaunchConfig,
  type PlanApprovalBlockerCode,
} from "./workflowBoard.logic";
import {
  WorkflowBoardChecksAndAcMapper,
  type DraftPlannedCheck,
} from "./WorkflowBoardChecksAndAcMapper";
import { WorkflowBoardLaunchControls } from "./WorkflowBoardLaunchControls";
import { WorkflowBoardPlanAlternatives } from "./WorkflowBoardPlanAlternatives";
import { WorkflowBoardPlanLenses } from "./WorkflowBoardPlanLenses";
import { WorkflowBoardTaskDagEditor } from "./WorkflowBoardTaskDagEditor";

const PLAN_BRIEF_KEY = WorkflowBoardArtifactKey.make("plan-brief.md");
const isValidPlanLensesArray = Schema.is(WorkflowBoardPlanLensesArray);
const isValidPlanAlternativesPair = Schema.is(WorkflowBoardPlanAlternativesPair);
const isValidTaskContractsArray = Schema.is(WorkflowBoardTaskContractsArray);
const isValidAcEvidenceMappingsArray = Schema.is(WorkflowBoardAcEvidenceMappingsArray);

interface PlanDraftState {
  readonly baseRevision: number;
  readonly lenses: ReadonlyArray<WorkflowBoardPlanLensEntry>;
  readonly alternatives: ReadonlyArray<WorkflowBoardPlanAlternative>;
  readonly selectedAlternativeId: string | null;
  readonly selectionRationale: string;
  readonly reviseAlternativesFeedback: string;
  readonly strategy: WorkflowBoardPlanningStrategy;
  readonly strategyRationale: string;
  readonly strategyMetadata: WorkflowBoardStrategyMetadata;
  readonly granularity: WorkflowBoardTaskGranularity;
  readonly tasks: ReadonlyArray<WorkflowBoardTaskContract>;
  readonly checks: ReadonlyArray<DraftPlannedCheck>;
  readonly acMappings: ReadonlyArray<WorkflowBoardAcEvidenceMapping>;
}

function buildPlanDraft(card: WorkflowCardDetail): PlanDraftState {
  const plan = card.plan ?? null;
  const existingLenses = plan?.lenses ?? card.lenses ?? [];
  const fullLenses = WORKFLOW_BOARD_PLAN_LENSES.map(
    (lens) =>
      existingLenses.find((entry) => entry.lens === lens) ?? {
        lens,
        applicable: null,
        summary: null,
        reason: null,
      },
  );
  return {
    baseRevision: card.revision,
    lenses: fullLenses,
    alternatives: plan?.alternatives ?? card.alternatives ?? [],
    selectedAlternativeId: plan?.selectedAlternativeId ?? card.selectedAlternativeId ?? null,
    selectionRationale: plan?.selectionRationale ?? card.selectionRationale ?? "",
    reviseAlternativesFeedback: "",
    strategy: plan?.strategy ?? card.strategy ?? "outcomes",
    strategyRationale: plan?.strategyRationale ?? card.strategyRationale ?? "",
    strategyMetadata: plan?.strategyMetadata ?? card.strategyMetadata ?? {},
    granularity: plan?.granularity ?? card.granularity ?? "balanced",
    tasks: plan?.tasks ?? card.tasks ?? [],
    checks: plan?.checks ?? [],
    acMappings: plan?.acMappings ?? card.acMappings ?? [],
  };
}

export function WorkflowBoardPlanEditor(props: {
  readonly environmentId: EnvironmentId;
  readonly workspaceRoot: string;
  readonly card: WorkflowCardDetail;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly existingWorktreePaths: ReadonlyArray<string>;
  readonly modelSelection: ModelSelection | null;
  readonly onModelSelectionChange: (next: ModelSelection) => void;
  readonly runtimeMode: RuntimeMode;
  readonly onRuntimeModeChange: (next: RuntimeMode) => void;
  readonly interactionMode: ProviderInteractionMode;
  readonly onInteractionModeChange: (next: ProviderInteractionMode) => void;
  readonly workspaceStrategy: WorkflowCardDetail["workspaceStrategy"];
  readonly onWorkspaceStrategyChange: (next: WorkflowCardDetail["workspaceStrategy"]) => void;
  readonly workspaceStrategyValid: boolean;
  readonly activePlanThreadId: ThreadId | null;
  readonly onPlanStarted?: (threadId: ThreadId) => void;
  readonly onActionError: (message: string | null) => void;
  readonly onActionNotice: (message: string | null) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const { card } = props;

  const canUpdatePlan = useAtomValue(workflowBoard.updatePlan.permissionAtom(props.environmentId));
  const canApprovePlan = useAtomValue(
    workflowBoard.approvePlan.permissionAtom(props.environmentId),
  );
  const canStartStage = useAtomValue(workflowBoard.startStage.permissionAtom(props.environmentId));

  const updatePlan = useAtomCommand(workflowBoard.updatePlan, {
    label: "workflow board update plan",
    reportFailure: false,
  });
  const approvePlan = useAtomCommand(workflowBoard.approvePlan, {
    label: "workflow board approve plan",
    reportFailure: false,
  });
  const startStage = useAtomCommand(workflowBoard.startStage, {
    label: "workflow board start stage",
    reportFailure: false,
  });

  const [draft, setDraft] = useState<PlanDraftState | null>(null);
  const [savingPlan, setSavingPlan] = useState(false);
  const [approvingPlan, setApprovingPlan] = useState(false);
  const [startingPlan, setStartingPlan] = useState(false);

  const activeDraft = draft ?? buildPlanDraft(card);
  const draftStaleVsServer = draft !== null && draft.baseRevision !== card.revision;

  const updateDraftField = (patch: Partial<PlanDraftState>) => {
    setDraft((prev) => ({
      ...(prev ?? buildPlanDraft(card)),
      ...patch,
    }));
  };

  const latestPlanRun = workflowBoardLatestStageRun(card, "plan");
  const hasPlanBrief = card.artifactKeys.includes(PLAN_BRIEF_KEY) || latestPlanRun !== null;
  const planBriefArtifact = useEnvironmentQuery(
    hasPlanBrief
      ? workflowBoard.artifact({
          environmentId: props.environmentId,
          input: { cardId: card.cardId, key: PLAN_BRIEF_KEY },
        })
      : null,
  );

  const launchDefaults = resolveDefaultBoardLaunchConfig({
    providers: props.providers,
    settings: props.settings,
    projectId: card.projectId,
    card,
  });
  const resumedPlanRun =
    latestPlanRun?.threadId !== null && latestPlanRun?.threadId === props.activePlanThreadId
      ? latestPlanRun
      : null;
  const effectiveModelSelection =
    resumedPlanRun?.modelSelection ?? props.modelSelection ?? launchDefaults.modelSelection;
  const runtimeMode = resumedPlanRun?.runtimeMode ?? props.runtimeMode;
  const interactionMode = resumedPlanRun?.interactionMode ?? props.interactionMode;
  const workspaceStrategy = resumedPlanRun?.workspaceStrategy ?? props.workspaceStrategy;
  const workspaceStrategyValid = isWorkflowBoardWorkspaceStrategyValid(workspaceStrategy);

  const canStartPlan = canStartWorkflowBoardPlan({
    detail: card,
    canOperate: canStartStage,
    workflowsEnabled: props.settings.workflowsEnabled,
  });
  const canApprove = canApproveWorkflowBoardPlan({
    detail: card,
    canOperate: canApprovePlan,
  });
  const blockers = collectPlanApprovalBlockers(card);
  const preservesSpec = isPlanOnlyEditPreservingSpecApproval(card);

  const strategyLabel = (strategy: WorkflowBoardPlanningStrategy): string => {
    switch (strategy) {
      case "outcomes":
        return gt("Outcomes", { $context: "workflow board planning strategy" });
      case "risk-first":
        return gt("Risk-first", { $context: "workflow board planning strategy" });
      case "expand-contract":
        return gt("Expand-contract", { $context: "workflow board planning strategy" });
      case "custom":
        return gt("Custom", { $context: "workflow board planning strategy" });
    }
  };

  const granularityLabel = (granularity: WorkflowBoardTaskGranularity): string => {
    switch (granularity) {
      case "coarse":
        return gt("Coarse", { $context: "workflow board task granularity" });
      case "balanced":
        return gt("Balanced", { $context: "workflow board task granularity" });
      case "fine":
        return gt("Fine", { $context: "workflow board task granularity" });
    }
  };

  const blockerLabel = (code: PlanApprovalBlockerCode): string => {
    switch (code) {
      case "spec_not_approved":
        return gt("Approve the spec first.");
      case "plan_not_saved":
        return gt("Save the plan to compute plan.md and design.md digests.");
      case "lenses_incomplete":
        return gt("Decide all 6 design lenses with summaries or reasons.");
      case "alternatives_incomplete":
        return gt("Provide 2 distinct alternatives, pick one, and record the selection rationale.");
      case "strategy_incomplete":
        return gt("Complete required planning strategy fields.");
      case "granularity_missing":
        return gt("Choose a task granularity.");
      case "tasks_incomplete":
        return gt("Add at least one valid task with exclusive file ownership.");
      case "checks_invalid":
        return gt("Fix invalid argv check commands.");
      case "ac_mappings_incomplete":
        return gt("Map 100% of acceptance criteria to automated, manual, or external evidence.");
    }
  };

  const handleSavePlan = async () => {
    if (!canUpdatePlan || savingPlan) return;
    props.onActionError(null);
    props.onActionNotice(null);

    if (!isValidPlanLensesArray(activeDraft.lenses)) {
      props.onActionError(gt("Complete every decided design lens with a summary or reason."));
      return;
    }
    const hasPartialAlts = activeDraft.alternatives.length > 0;
    if (hasPartialAlts && !isValidPlanAlternativesPair(activeDraft.alternatives)) {
      props.onActionError(
        gt("Provide two materially distinct alternatives with non-empty fields."),
      );
      return;
    }
    if (activeDraft.selectedAlternativeId !== null) {
      const selectedTrimmed = activeDraft.selectedAlternativeId.trim();
      const matchesAlternative =
        activeDraft.alternatives.length === 2 &&
        activeDraft.alternatives.some((alt) => alt.id.trim() === selectedTrimmed);
      if (!matchesAlternative) {
        props.onActionError(
          gt("Selected alternative must match one of the two compared alternatives."),
        );
        return;
      }
      if (activeDraft.selectionRationale.trim() === "") {
        props.onActionError(gt("Selection rationale is required when selecting an alternative."));
        return;
      }
    }
    if (
      !isPlanStrategyValid({
        strategy: activeDraft.strategy,
        strategyRationale: activeDraft.strategyRationale,
        strategyMetadata: activeDraft.strategyMetadata,
      })
    ) {
      props.onActionError(gt("Complete the required strategy rationale or metadata fields."));
      return;
    }
    if (activeDraft.tasks.length > 0 && !isValidTaskContractsArray(activeDraft.tasks)) {
      props.onActionError(gt("Fix task DAG cycles or overlapping allowed files before saving."));
      return;
    }
    if (activeDraft.checks.some((check) => check.id.trim() === "")) {
      props.onActionError(gt("Every planned check requires a non-empty ID."));
      return;
    }
    if (activeDraft.checks.some((check) => !isSafeWorkflowBoardArgv(check.argv))) {
      props.onActionError(gt("Planned checks must be argv-only without shell operators."));
      return;
    }
    const normalizedAcMappings = normalizeAcEvidenceMappings(activeDraft.acMappings);
    if (normalizedAcMappings.length > 0 && !isValidAcEvidenceMappingsArray(normalizedAcMappings)) {
      props.onActionError(gt("Complete all acceptance criterion evidence mappings before saving."));
      return;
    }

    const trimmedFeedback = activeDraft.reviseAlternativesFeedback.trim();
    const strategyMeta = activeDraft.strategyMetadata;
    const strategyPayload =
      activeDraft.strategy === "risk-first"
        ? {
            strategy: "risk-first" as const,
            strategyMetadata: {
              uncertainty: strategyMeta.uncertainty?.trim() ?? "",
              probeTaskId: strategyMeta.probeTaskId?.trim() ?? "",
            },
          }
        : activeDraft.strategy === "expand-contract"
          ? {
              strategy: "expand-contract" as const,
              strategyMetadata: {
                compatibilityWindow: strategyMeta.compatibilityWindow?.trim() ?? "",
                transitionStep: strategyMeta.transitionStep?.trim() ?? "",
                retirementPrereq: strategyMeta.retirementPrereq?.trim() ?? "",
              },
            }
          : activeDraft.strategy === "custom"
            ? {
                strategy: "custom" as const,
                strategyRationale: activeDraft.strategyRationale.trim(),
              }
            : {
                strategy: "outcomes" as const,
                ...(activeDraft.strategyRationale.trim() !== ""
                  ? { strategyRationale: activeDraft.strategyRationale.trim() }
                  : {}),
              };

    setSavingPlan(true);
    const result = await updatePlan({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        lenses: activeDraft.lenses,
        ...(activeDraft.alternatives.length === 2
          ? { alternatives: activeDraft.alternatives }
          : {}),
        selectedAlternativeId: activeDraft.selectedAlternativeId,
        ...(activeDraft.selectedAlternativeId !== null
          ? { selectionRationale: activeDraft.selectionRationale.trim() }
          : {}),
        ...(trimmedFeedback !== "" ? { reviseAlternativesFeedback: trimmedFeedback } : {}),
        ...strategyPayload,
        granularity: activeDraft.granularity,
        tasks: activeDraft.tasks,
        checks: activeDraft.checks.map(({ id, criterionId, argv, description }) => ({
          id: id.trim(),
          ...(criterionId !== undefined ? { criterionId } : {}),
          argv,
          ...(description !== undefined ? { description } : {}),
        })),
        acMappings: normalizedAcMappings,
      },
    });
    setSavingPlan(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not save plan."),
      );
      return;
    }

    setDraft(null);
    props.onActionNotice(
      preservesSpec
        ? gt("Plan saved at revision {revision}. Spec approval preserved.", {
            revision: result.value.revision,
          })
        : gt("Plan saved at revision {revision}.", { revision: result.value.revision }),
    );
  };

  const handleApprovePlan = async () => {
    if (!canApprove || approvingPlan) return;
    setApprovingPlan(true);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await approvePlan({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        ...(card.spec.specDigest ? { expectedSpecDigest: card.spec.specDigest } : {}),
        ...(card.plan?.planDigest ? { expectedPlanDigest: card.plan.planDigest } : {}),
      },
    });
    setApprovingPlan(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not approve plan."),
      );
      return;
    }
    props.onActionNotice(
      gt("Plan approved at revision {revision}.", { revision: result.value.revision }),
    );
  };

  const handleStartPlan = async () => {
    if (
      !canStartPlan ||
      startingPlan ||
      effectiveModelSelection === null ||
      !workspaceStrategyValid
    ) {
      return;
    }
    setStartingPlan(true);
    props.onActionError(null);
    props.onActionNotice(null);

    const result = await startStage({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
        stage: "plan",
        modelSelection: effectiveModelSelection,
        runtimeMode,
        interactionMode,
        workspaceStrategy: normalizeWorkspaceStrategy(workspaceStrategy),
      },
    });
    setStartingPlan(false);

    if (result._tag === "Failure") {
      props.onActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not start Plan stage."),
      );
      return;
    }

    props.onPlanStarted?.(result.value.threadId);
    props.onActionNotice(
      result.value.resumed
        ? gt("Resumed existing Plan thread.")
        : gt("Started Plan stage and prepared Plan brief."),
    );
  };

  const planApproved = card.planApproval?.status === "valid";

  return (
    <div className="flex flex-col gap-5">
      {preservesSpec ? (
        <Alert variant="success" role="note">
          <CheckCircle2Icon />
          <AlertTitle>
            <T>Plan-only edit preserves Spec approval</T>
          </AlertTitle>
          <AlertDescription>
            <T>
              Editing plan fields updates the plan revision while keeping the approved spec intact.
            </T>
          </AlertDescription>
        </Alert>
      ) : null}

      {draftStaleVsServer ? (
        <Alert variant="warning" role="note">
          <AlertTitle>
            <T>Newer server revision available</T>
          </AlertTitle>
          <AlertDescription>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span>
                <T>
                  Your local plan draft started at revision <Var>{draft.baseRevision}</Var>, while
                  the server is at revision <Var>{card.revision}</Var>.
                </T>
              </span>
              <Button type="button" size="xs" variant="outline" onClick={() => setDraft(null)}>
                <T>Reset to server</T>
              </Button>
            </div>
          </AlertDescription>
        </Alert>
      ) : null}

      <section
        aria-labelledby="plan-launch-heading"
        className="flex flex-col gap-3 rounded-lg border bg-card p-4"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 id="plan-launch-heading" className="text-sm font-semibold text-foreground">
              <T>Plan stage handoff</T>
            </h3>
            <p className="text-xs text-muted-foreground">
              {card.specApproval?.status === "valid" ? (
                <T>Start Plan to prepare the brief and connect a worktree-aware agent thread.</T>
              ) : (
                <T>Approve the spec before starting Plan.</T>
              )}
            </p>
          </div>
          <Button
            type="button"
            size="sm"
            disabled={
              !canStartPlan ||
              startingPlan ||
              effectiveModelSelection === null ||
              !workspaceStrategyValid
            }
            onClick={() => void handleStartPlan()}
          >
            <PlayIcon />
            {startingPlan ? (
              <T>Starting Plan…</T>
            ) : props.activePlanThreadId !== null ? (
              <T>Resume Plan</T>
            ) : (
              <T>Start Plan</T>
            )}
          </Button>
        </div>

        <WorkflowBoardLaunchControls
          environmentId={props.environmentId}
          projectId={card.projectId}
          providers={props.providers}
          settings={props.settings}
          existingWorktreePaths={props.existingWorktreePaths}
          modelSelection={effectiveModelSelection}
          onModelSelectionChange={props.onModelSelectionChange}
          runtimeMode={runtimeMode}
          onRuntimeModeChange={props.onRuntimeModeChange}
          interactionMode={interactionMode}
          onInteractionModeChange={props.onInteractionModeChange}
          workspaceStrategy={workspaceStrategy}
          onWorkspaceStrategyChange={props.onWorkspaceStrategyChange}
          disabled={startingPlan || !canStartStage || resumedPlanRun !== null}
        />
      </section>

      {hasPlanBrief ? (
        <section aria-labelledby="plan-brief-heading" className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-2">
            <h3 id="plan-brief-heading" className="text-sm font-semibold text-foreground">
              <T>Prepared Plan brief</T>
            </h3>
            {planBriefArtifact.data !== null ? (
              <span className="font-mono text-xs text-muted-foreground">
                {planBriefArtifact.data.relativePath}
              </span>
            ) : null}
          </div>
          {planBriefArtifact.error !== null ? (
            <Alert variant="error" role="alert">
              <AlertDescription>
                <p>{planBriefArtifact.error}</p>
                <Button size="xs" variant="outline" onClick={planBriefArtifact.refresh}>
                  <T>Retry</T>
                </Button>
              </AlertDescription>
            </Alert>
          ) : planBriefArtifact.data === null ? (
            <p className="text-xs text-muted-foreground" role="status">
              <T>Loading Plan brief…</T>
            </p>
          ) : (
            <div className="rounded-lg border bg-card p-4">
              <ChatMarkdown
                text={planBriefArtifact.data.content}
                cwd={props.workspaceRoot}
                environmentId={props.environmentId}
              />
            </div>
          )}
        </section>
      ) : null}

      <section
        aria-labelledby="plan-authoring-heading"
        className="flex flex-col gap-4 rounded-lg border bg-card p-4"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <h3 id="plan-authoring-heading" className="text-sm font-semibold text-foreground">
              <T>Plan authoring</T>
            </h3>
            {card.plan?.planDigest ? (
              <Badge variant="outline" size="sm">
                {card.plan.planDigest}
              </Badge>
            ) : null}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={!canUpdatePlan || savingPlan}
              onClick={() => void handleSavePlan()}
            >
              <SaveIcon />
              {savingPlan ? <T>Saving plan…</T> : <T>Save plan</T>}
            </Button>
            <Button
              type="button"
              size="sm"
              disabled={!canApprove || approvingPlan || planApproved}
              onClick={() => void handleApprovePlan()}
            >
              <CheckCircle2Icon />
              {approvingPlan ? (
                <T>Approving…</T>
              ) : planApproved ? (
                <T>Plan approved</T>
              ) : (
                <T>Approve plan</T>
              )}
            </Button>
          </div>
        </div>

        <WorkflowBoardPlanLenses
          lenses={activeDraft.lenses}
          disabled={!canUpdatePlan || savingPlan}
          onChange={(lenses) => updateDraftField({ lenses })}
        />

        <WorkflowBoardPlanAlternatives
          alternatives={activeDraft.alternatives}
          alternativesRevisionCount={
            card.plan?.alternativesRevisionCount ?? card.alternativesRevisionCount ?? 0
          }
          selectedAlternativeId={activeDraft.selectedAlternativeId}
          selectionRationale={activeDraft.selectionRationale}
          reviseAlternativesFeedback={activeDraft.reviseAlternativesFeedback}
          disabled={!canUpdatePlan || savingPlan}
          onAlternativesChange={(alternatives) => updateDraftField({ alternatives })}
          onSelectAlternative={(selectedAlternativeId) =>
            updateDraftField({ selectedAlternativeId })
          }
          onRationaleChange={(selectionRationale) => updateDraftField({ selectionRationale })}
          onReviseFeedbackChange={(reviseAlternativesFeedback) =>
            updateDraftField({ reviseAlternativesFeedback })
          }
        />

        <div className="grid gap-3 border-t pt-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">
              <T>Planning strategy</T>
            </span>
            <Select
              value={activeDraft.strategy}
              disabled={!canUpdatePlan || savingPlan}
              onValueChange={(value) => {
                if (
                  value === "outcomes" ||
                  value === "risk-first" ||
                  value === "expand-contract" ||
                  value === "custom"
                ) {
                  updateDraftField({ strategy: value });
                }
              }}
            >
              <SelectTrigger size="sm" aria-label={gt("Planning strategy")}>
                <SelectValue>{strategyLabel(activeDraft.strategy)}</SelectValue>
              </SelectTrigger>
              <SelectPopup>
                <SelectItem value="outcomes">{strategyLabel("outcomes")}</SelectItem>
                <SelectItem value="risk-first">{strategyLabel("risk-first")}</SelectItem>
                <SelectItem value="expand-contract">{strategyLabel("expand-contract")}</SelectItem>
                <SelectItem value="custom">{strategyLabel("custom")}</SelectItem>
              </SelectPopup>
            </Select>
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">
              <T>Task granularity</T>
            </span>
            <Select
              value={activeDraft.granularity}
              disabled={!canUpdatePlan || savingPlan}
              onValueChange={(value) => {
                if (value === "coarse" || value === "balanced" || value === "fine") {
                  updateDraftField({ granularity: value });
                }
              }}
            >
              <SelectTrigger size="sm" aria-label={gt("Task granularity")}>
                <SelectValue>{granularityLabel(activeDraft.granularity)}</SelectValue>
              </SelectTrigger>
              <SelectPopup>
                <SelectItem value="coarse">{granularityLabel("coarse")}</SelectItem>
                <SelectItem value="balanced">{granularityLabel("balanced")}</SelectItem>
                <SelectItem value="fine">{granularityLabel("fine")}</SelectItem>
              </SelectPopup>
            </Select>
          </div>
        </div>

        {activeDraft.strategy === "custom" ? (
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="plan-strategy-rationale"
              className="text-xs font-medium text-muted-foreground"
            >
              <T>Custom strategy rationale</T>
            </label>
            <Textarea
              id="plan-strategy-rationale"
              rows={2}
              value={activeDraft.strategyRationale}
              disabled={!canUpdatePlan || savingPlan}
              onChange={(event) => updateDraftField({ strategyRationale: event.target.value })}
            />
          </div>
        ) : activeDraft.strategy === "risk-first" ? (
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="plan-risk-uncertainty"
                className="text-xs font-medium text-muted-foreground"
              >
                <T>Key uncertainty</T>
              </label>
              <Input
                id="plan-risk-uncertainty"
                size="sm"
                value={activeDraft.strategyMetadata.uncertainty ?? ""}
                disabled={!canUpdatePlan || savingPlan}
                onValueChange={(value) =>
                  updateDraftField({
                    strategyMetadata: { ...activeDraft.strategyMetadata, uncertainty: value },
                  })
                }
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label
                htmlFor="plan-risk-probe"
                className="text-xs font-medium text-muted-foreground"
              >
                <T>Probe task ID</T>
              </label>
              <Input
                id="plan-risk-probe"
                size="sm"
                font="mono"
                value={activeDraft.strategyMetadata.probeTaskId ?? ""}
                disabled={!canUpdatePlan || savingPlan}
                onValueChange={(value) =>
                  updateDraftField({
                    strategyMetadata: { ...activeDraft.strategyMetadata, probeTaskId: value },
                  })
                }
              />
            </div>
          </div>
        ) : activeDraft.strategy === "expand-contract" ? (
          <div className="grid gap-3 sm:grid-cols-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="plan-ec-window" className="text-xs font-medium text-muted-foreground">
                <T>Compatibility window</T>
              </label>
              <Input
                id="plan-ec-window"
                size="sm"
                value={activeDraft.strategyMetadata.compatibilityWindow ?? ""}
                disabled={!canUpdatePlan || savingPlan}
                onValueChange={(value) =>
                  updateDraftField({
                    strategyMetadata: {
                      ...activeDraft.strategyMetadata,
                      compatibilityWindow: value,
                    },
                  })
                }
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label htmlFor="plan-ec-step" className="text-xs font-medium text-muted-foreground">
                <T>Transition step</T>
              </label>
              <Input
                id="plan-ec-step"
                size="sm"
                value={activeDraft.strategyMetadata.transitionStep ?? ""}
                disabled={!canUpdatePlan || savingPlan}
                onValueChange={(value) =>
                  updateDraftField({
                    strategyMetadata: { ...activeDraft.strategyMetadata, transitionStep: value },
                  })
                }
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label htmlFor="plan-ec-retire" className="text-xs font-medium text-muted-foreground">
                <T>Retirement prerequisite</T>
              </label>
              <Input
                id="plan-ec-retire"
                size="sm"
                value={activeDraft.strategyMetadata.retirementPrereq ?? ""}
                disabled={!canUpdatePlan || savingPlan}
                onValueChange={(value) =>
                  updateDraftField({
                    strategyMetadata: { ...activeDraft.strategyMetadata, retirementPrereq: value },
                  })
                }
              />
            </div>
          </div>
        ) : null}

        <WorkflowBoardTaskDagEditor
          tasks={activeDraft.tasks}
          disabled={!canUpdatePlan || savingPlan}
          onChange={(tasks) => updateDraftField({ tasks })}
        />

        <WorkflowBoardChecksAndAcMapper
          checks={activeDraft.checks}
          acceptanceCriteria={card.spec.acceptanceCriteria}
          acMappings={activeDraft.acMappings}
          disabled={!canUpdatePlan || savingPlan}
          onChecksChange={(checks) => updateDraftField({ checks })}
          onAcMappingsChange={(acMappings) => updateDraftField({ acMappings })}
        />

        {blockers.length > 0 && !planApproved ? (
          <div className="rounded-md border bg-muted/20 p-3 text-xs">
            <p className="font-medium text-foreground">
              <T>Remaining Plan approval gates</T>
            </p>
            <ul className="mt-1 list-disc space-y-0.5 ps-4 text-muted-foreground">
              {blockers.map((code) => (
                <li key={code}>{blockerLabel(code)}</li>
              ))}
            </ul>
          </div>
        ) : null}
      </section>
    </div>
  );
}

import { useAtomValue } from "@effect/atom-react";
import { scopeThreadRef } from "@legioncode/client-runtime/environment";
import {
  isWorkflowBoardCardShipped,
  workflowBoardCardStatusLabel,
  workflowBoardCliImportHistoricalReceiptKindLabel,
  workflowBoardCliImportPolicyStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardErrorMessage,
  workflowBoardLatestStageRun,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
} from "@legioncode/client-runtime/workflow-board";
import {
  PositiveInt,
  type EnvironmentId,
  type ModelSelection,
  type ProjectId,
  type ProviderInteractionMode,
  type RuntimeMode,
  type ServerProvider,
  type ServerSettings,
  type ThreadId,
  type WorkflowBoardCardSummary,
  type WorkflowBoardFixPointer,
  type WorkflowBoardStage,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import * as Cause from "effect/Cause";
import { T, useGT, useMessages, Var } from "gt-react";
import { ArrowLeftIcon, CheckCircle2Icon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";

import { workspaceAreaProps } from "../../lib/workspaceFocus";
import { useEnvironmentQuery } from "../../state/query";
import { useAtomCommand } from "../../state/use-atom-command";
import { workflowBoard } from "../../state/workflowBoard";
import { Alert, AlertDescription, AlertTitle } from "../ui/alert";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import WorkflowThreadPane from "../workflows/WorkflowThreadPane";
import {
  canApproveWorkflowBoardSpec,
  collectDownstreamInvalidationStates,
  isWorkflowBoardWorkspaceStrategyValid,
  resolveCrossColumnTransition,
  resolveDefaultBoardLaunchConfig,
  resolveFixPointerNavigation,
  validReturnTargetStages,
  type WorkflowBoardCrossColumnTransition,
} from "./workflowBoard.logic";
import { WorkflowBoardChallengePanel } from "./WorkflowBoardChallengePanel";
import { WorkflowBoardExecutePanel } from "./WorkflowBoardExecutePanel";
import { WorkflowBoardExplorationWizard } from "./WorkflowBoardExplorationWizard";
import { WorkflowBoardLaunchControls } from "./WorkflowBoardLaunchControls";
import { WorkflowBoardPlanEditor } from "./WorkflowBoardPlanEditor";
import { WorkflowBoardShipPanel } from "./WorkflowBoardShipPanel";
import { WorkflowBoardSpecEditor } from "./WorkflowBoardSpecEditor";
import { WorkflowBoardWireframePreview } from "./WorkflowBoardWireframePreview";

type WorkspaceSubTab = "spec" | "plan" | "execute" | "ship" | "history";

const WORKSPACE_SUB_TABS: readonly WorkspaceSubTab[] = [
  "spec",
  "plan",
  "execute",
  "ship",
  "history",
];

interface PointerFocusRequest {
  readonly nonce: number;
  readonly section:
    | "spec"
    | "plan"
    | "recovery"
    | "policies"
    | "tasks"
    | "checks"
    | "review"
    | "acceptance";
  readonly targetId: string | null;
}

function escapeAttrSelector(value: string): string {
  return typeof CSS !== "undefined" && typeof CSS.escape === "function"
    ? CSS.escape(value)
    : value.replace(/["\\]/g, "\\$&");
}

function findPointerTargetElement(
  root: HTMLElement,
  request: PointerFocusRequest,
): HTMLElement | null {
  if (request.targetId !== null) {
    const escaped = escapeAttrSelector(request.targetId);
    const selectors = [
      `[data-check-id="${escaped}"]`,
      `[data-execute-task-id="${escaped}"]`,
      `[data-review-finding-id="${escaped}"]`,
      `[data-cli-policy-key="${escaped}"]`,
      `[data-ac-criterion-id="${escaped}"]`,
    ];
    for (const selector of selectors) {
      const match = root.querySelector<HTMLElement>(selector);
      if (match !== null) return match;
    }
  }
  return (
    root.querySelector<HTMLElement>(`[data-execute-section="${request.section}"]`) ??
    root.querySelector<HTMLElement>(`[data-ship-section="${request.section}"]`) ??
    root.querySelector<HTMLElement>(`[data-workspace-section="${request.section}"]`)
  );
}

function toCardSummary(card: WorkflowCardDetail): WorkflowBoardCardSummary {
  return {
    cardId: card.cardId,
    projectId: card.projectId,
    title: card.title,
    stage: card.stage,
    status: card.status,
    revision: card.revision,
    currentTask: card.currentTask,
    blockerSummary: card.blockerSummary,
    nextAction: card.nextAction,
    specApprovedRevision: card.specApprovedRevision,
    specApprovalStatus: card.specApprovalStatus,
    activeThreadId: card.activeThreadId,
    createdAt: card.createdAt,
    updatedAt: card.updatedAt,
    planApprovedRevision: card.planApprovedRevision,
    planApprovalStatus: card.planApprovalStatus,
    incompleteJournalCount: card.incompleteJournalCount,
    hasUnreconciledEdits: card.hasUnreconciledEdits,
    deliveryStatus: card.deliveryStatus,
  };
}

export function WorkflowBoardCardWorkspace(props: {
  readonly environmentId: EnvironmentId;
  readonly projectId: ProjectId;
  readonly workspaceRoot: string;
  readonly card: WorkflowCardDetail;
  readonly requestedStage?: WorkflowBoardStage | undefined;
  readonly inspectShipNonce?: number | undefined;
  readonly providers: ReadonlyArray<ServerProvider>;
  readonly settings: ServerSettings;
  readonly existingWorktreePaths: ReadonlyArray<string>;
  readonly onStageStarted?: (threadId: ThreadId) => void;
  readonly onCardRefresh?: () => void;
  readonly onRequestTransition?: (
    card: WorkflowBoardCardSummary,
    transition: WorkflowBoardCrossColumnTransition,
  ) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const specStaleTitleId = useId();
  const planStaleTitleId = useId();
  const externalEditTitleId = useId();
  const workspaceContainerRef = useRef<HTMLDivElement | null>(null);
  const { card } = props;

  const canReadBoard = useAtomValue(workflowBoard.readPermissionAtom(props.environmentId));
  const canApproveSpec = useAtomValue(
    workflowBoard.approveSpec.permissionAtom(props.environmentId),
  );
  const canReturn = useAtomValue(
    workflowBoard.returnForRevision.permissionAtom(props.environmentId),
  );

  const approveSpec = useAtomCommand(workflowBoard.approveSpec, {
    label: "workflow board approve spec",
    reportFailure: false,
  });

  const launchDefaults = resolveDefaultBoardLaunchConfig({
    providers: props.providers,
    settings: props.settings,
    projectId: props.projectId,
    card,
  });

  const [selectedSubTab, setSelectedSubTab] = useState<{
    readonly stage: WorkflowBoardStage;
    readonly tab: WorkspaceSubTab;
  } | null>(null);
  const [modelSelection, setModelSelection] = useState<ModelSelection | null>(null);
  const [runtimeMode, setRuntimeMode] = useState<RuntimeMode>(launchDefaults.runtimeMode);
  const [interactionMode, setInteractionMode] = useState<ProviderInteractionMode>(
    launchDefaults.interactionMode,
  );
  const [workspaceStrategy, setWorkspaceStrategy] = useState(launchDefaults.workspaceStrategy);
  const [approvingSpec, setApprovingSpec] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [startedSpecThreadId, setStartedSpecThreadId] = useState<ThreadId | null>(null);
  const [startedPlanThreadId, setStartedPlanThreadId] = useState<ThreadId | null>(null);
  const [startedExecuteThreadId, setStartedExecuteThreadId] = useState<ThreadId | null>(null);
  const [startedReviewThreadId, setStartedReviewThreadId] = useState<ThreadId | null>(null);
  const [selectedThreadOverride, setSelectedThreadOverride] = useState<{
    readonly stage: "execute" | "review";
    readonly threadId: ThreadId;
  } | null>(null);
  const [executeSectionFocus, setExecuteSectionFocus] = useState<{
    readonly section: "recovery" | "policies" | "tasks" | "checks" | "review";
    readonly targetId: string | null;
  } | null>(null);
  const [shipHighlightTargetId, setShipHighlightTargetId] = useState<string | null>(null);
  const [pointerFocusRequest, setPointerFocusRequest] = useState<PointerFocusRequest | null>(null);
  const [prevInspectShipNonce, setPrevInspectShipNonce] = useState(props.inspectShipNonce ?? 0);

  if ((props.inspectShipNonce ?? 0) !== prevInspectShipNonce) {
    setPrevInspectShipNonce(props.inspectShipNonce ?? 0);
    setSelectedSubTab({ stage: card.stage, tab: "ship" });
  }

  const defaultSubTab: WorkspaceSubTab =
    card.stage === "ship" || card.status === "completed" || props.requestedStage === "ship"
      ? "ship"
      : card.stage === "execute" || props.requestedStage === "execute"
        ? "execute"
        : card.stage === "plan" || props.requestedStage === "plan"
          ? "plan"
          : "spec";
  const activeSubTab: WorkspaceSubTab =
    selectedSubTab !== null && selectedSubTab.stage === card.stage
      ? selectedSubTab.tab
      : defaultSubTab;

  // oxlint-disable-next-line legioncode/no-unreasoned-state-sync -- scrolls and focuses the target DOM node inside the workspace after Go to fix navigation
  useEffect(() => {
    if (pointerFocusRequest === null) return;
    const root = workspaceContainerRef.current;
    if (root === null) return;
    const target = findPointerTargetElement(root, pointerFocusRequest);
    if (target === null) return;
    const reducedMotion =
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    target.scrollIntoView({
      block: "nearest",
      behavior: reducedMotion ? "auto" : "smooth",
    });
    target.focus({ preventScroll: true });
  }, [pointerFocusRequest]);

  // oxlint-disable-next-line legioncode/no-unreasoned-state-sync -- scrolls and focuses the Ship readiness DOM section after Inspect Ship blockers
  useEffect(() => {
    if (!props.inspectShipNonce || activeSubTab !== "ship") return;
    const root = workspaceContainerRef.current;
    const target = root?.querySelector<HTMLElement>('[data-ship-section="readiness"]') ?? null;
    if (target === null) return;
    const reducedMotion =
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    target.scrollIntoView({
      block: "nearest",
      behavior: reducedMotion ? "auto" : "smooth",
    });
    target.focus({ preventScroll: true });
  }, [props.inspectShipNonce, activeSubTab]);

  const cardDiffQuery = useEnvironmentQuery(
    canReadBoard && (activeSubTab === "execute" || activeSubTab === "ship")
      ? workflowBoard.cardDiff({
          environmentId: props.environmentId,
          input: { cardId: card.cardId, includePatches: true },
        })
      : null,
  );

  const latestSpecRun = workflowBoardLatestStageRun(card, "spec");
  const latestPlanRun = workflowBoardLatestStageRun(card, "plan");
  const latestExecuteRun = workflowBoardLatestStageRun(card, "execute");
  const latestReviewRun = workflowBoardLatestStageRun(card, "review");

  const activeSpecThreadId = startedSpecThreadId ?? latestSpecRun?.threadId ?? null;
  const activePlanThreadId =
    startedPlanThreadId ?? latestPlanRun?.threadId ?? card.activeThreadId ?? null;
  const activeExecuteThreadId =
    startedExecuteThreadId ??
    latestExecuteRun?.threadId ??
    card.taskExecutions?.find((exec) => exec.threadId !== null)?.threadId ??
    null;
  const activeReviewThreadId =
    startedReviewThreadId ?? card.reviewReport?.threadId ?? latestReviewRun?.threadId ?? null;

  const displayedThreadKind: "spec" | "plan" | "execute" | "review" =
    selectedThreadOverride !== null
      ? selectedThreadOverride.stage
      : activeSubTab === "spec"
        ? activeSpecThreadId !== null
          ? "spec"
          : "plan"
        : activeSubTab === "plan"
          ? activePlanThreadId !== null
            ? "plan"
            : "spec"
          : activeSubTab === "execute"
            ? activeExecuteThreadId !== null
              ? "execute"
              : activeReviewThreadId !== null
                ? "review"
                : "plan"
            : activeReviewThreadId !== null
              ? "review"
              : activeExecuteThreadId !== null
                ? "execute"
                : activePlanThreadId !== null
                  ? "plan"
                  : "spec";

  const displayedThreadId =
    selectedThreadOverride !== null
      ? selectedThreadOverride.threadId
      : activeSubTab === "spec"
        ? (activeSpecThreadId ?? activePlanThreadId)
        : activeSubTab === "plan"
          ? (activePlanThreadId ?? activeSpecThreadId)
          : activeSubTab === "execute"
            ? (activeExecuteThreadId ?? activeReviewThreadId ?? activePlanThreadId)
            : (activeReviewThreadId ??
              activeExecuteThreadId ??
              activePlanThreadId ??
              activeSpecThreadId);

  const specApprovalState = workflowBoardSpecApprovalState(card);
  const specBadgeVariant =
    specApprovalState === "approved"
      ? "success"
      : specApprovalState === "stale"
        ? "warning"
        : "secondary";
  const planApprovalState = workflowBoardPlanApprovalState(card);
  const planBadgeVariant =
    planApprovalState === "approved"
      ? "success"
      : planApprovalState === "stale"
        ? "warning"
        : "secondary";

  const canApprove = canApproveWorkflowBoardSpec({
    detail: card,
    canOperate: canApproveSpec,
  });
  const effectiveModelSelection = modelSelection ?? launchDefaults.modelSelection;
  const workspaceStrategyValid = isWorkflowBoardWorkspaceStrategyValid(workspaceStrategy);
  const invalidation = collectDownstreamInvalidationStates(card);
  const returnTargets = validReturnTargetStages(card.stage);
  const defaultReturnTarget = returnTargets[returnTargets.length - 1] ?? null;
  const returnTransition =
    defaultReturnTarget !== null
      ? resolveCrossColumnTransition(toCardSummary(card), defaultReturnTarget)
      : null;

  const handleApproveSpec = async () => {
    if (!canApprove || approvingSpec) return;
    setApprovingSpec(true);
    setActionError(null);
    setActionNotice(null);

    const result = await approveSpec({
      environmentId: props.environmentId,
      input: {
        cardId: card.cardId,
        expectedRevision: PositiveInt.make(card.revision),
      },
    });
    setApprovingSpec(false);

    if (result._tag === "Failure") {
      setActionError(
        workflowBoardErrorMessage(Cause.squash(result.cause), messages) ??
          gt("Could not approve spec."),
      );
      return;
    }
    setActionNotice(
      gt("Spec approved at revision {revision}.", { revision: result.value.revision }),
    );
  };

  const handleNavigateToPointer = (pointer: WorkflowBoardFixPointer) => {
    const nav = resolveFixPointerNavigation(pointer);
    const nextNonce = (pointerFocusRequest?.nonce ?? 0) + 1;
    setSelectedSubTab({ stage: card.stage, tab: nav.subTab });
    if (nav.subTab === "execute") {
      const section = nav.executeSection ?? "tasks";
      setExecuteSectionFocus({
        section,
        targetId: nav.targetId,
      });
      setShipHighlightTargetId(null);
      setPointerFocusRequest({ nonce: nextNonce, section, targetId: nav.targetId });
      return;
    }
    if (nav.subTab === "ship") {
      setShipHighlightTargetId(nav.targetId);
      setExecuteSectionFocus(null);
      setPointerFocusRequest({
        nonce: nextNonce,
        section: "acceptance",
        targetId: nav.targetId,
      });
      return;
    }
    setExecuteSectionFocus(null);
    setShipHighlightTargetId(null);
    setPointerFocusRequest({
      nonce: nextNonce,
      section: nav.subTab === "plan" ? "plan" : "spec",
      targetId: nav.targetId,
    });
  };

  const handleSubTabListKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const currentIndex = WORKSPACE_SUB_TABS.indexOf(activeSubTab);
    let nextTab: WorkspaceSubTab | undefined;
    if (event.key === "ArrowRight") {
      nextTab = WORKSPACE_SUB_TABS[(currentIndex + 1) % WORKSPACE_SUB_TABS.length];
    } else if (event.key === "ArrowLeft") {
      nextTab =
        WORKSPACE_SUB_TABS[
          (currentIndex - 1 + WORKSPACE_SUB_TABS.length) % WORKSPACE_SUB_TABS.length
        ];
    } else if (event.key === "Home") {
      nextTab = WORKSPACE_SUB_TABS[0];
    } else if (event.key === "End") {
      nextTab = WORKSPACE_SUB_TABS[WORKSPACE_SUB_TABS.length - 1];
    }
    if (nextTab === undefined) return;
    event.preventDefault();
    setSelectedSubTab({ stage: card.stage, tab: nextTab });
    document.getElementById(`card-workspace-tab-${nextTab}`)?.focus();
  };

  const revisions = card.history ?? card.revisions;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden lg:flex-row">
      <div
        ref={workspaceContainerRef}
        {...workspaceAreaProps("board-card-workspace", 2, gt("Card workspace"))}
        className="scrollbar-gutter-both flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto p-4"
      >
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-5">
          <div className="flex flex-wrap items-start justify-between gap-3 border-b pb-4">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="truncate text-lg font-semibold text-foreground">{card.title}</h2>
                <Badge variant="outline">{workflowBoardStageLabel(card.stage, messages)}</Badge>
                <Badge variant={card.status === "blocked" ? "error" : "secondary"}>
                  {workflowBoardCardStatusLabel(card.status, messages)}
                </Badge>
                <Badge variant={specBadgeVariant}>
                  {workflowBoardSpecApprovalLabel(card.specApprovalStatus, messages)}
                </Badge>
                <Badge variant={planBadgeVariant}>
                  {workflowBoardPlanApprovalLabel(card.planApprovalStatus, messages)}
                </Badge>
                {card.deliveryStatus !== undefined && card.deliveryStatus !== null ? (
                  <Badge
                    variant={
                      isWorkflowBoardCardShipped(card)
                        ? "success"
                        : card.deliveryStatus === "ready"
                          ? "success"
                          : card.deliveryStatus === "blocked"
                            ? "error"
                            : "secondary"
                    }
                  >
                    {workflowBoardDeliveryStatusLabel(card.deliveryStatus, messages)}
                  </Badge>
                ) : null}
                {card.cliImport !== undefined ? (
                  <>
                    <Badge variant="info" size="sm">
                      <T>
                        Imported from <Var>{card.cliImport.sourceCliDir}</Var>
                      </T>
                    </Badge>
                    <Badge variant="outline" size="sm">
                      <T>Managed by LegionCode</T>
                    </Badge>
                  </>
                ) : null}
              </div>
              <p className="mt-1 font-mono text-xs text-muted-foreground">
                <T>
                  Card <Var>{card.cardId}</Var> · rev <Var>{card.revision}</Var>
                </T>
                {card.spec.specDigest ? (
                  <>
                    {" "}
                    · <Var>{card.spec.specDigest}</Var>
                  </>
                ) : null}
                {card.plan?.planDigest ? (
                  <>
                    {" "}
                    · <Var>{card.plan.planDigest}</Var>
                  </>
                ) : null}
              </p>
            </div>

            {returnTransition !== null && props.onRequestTransition !== undefined ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={!canReturn}
                onClick={() => props.onRequestTransition?.(toCardSummary(card), returnTransition)}
              >
                <ArrowLeftIcon />
                <T>Return for revision</T>
              </Button>
            ) : null}
          </div>

          <div
            role="tablist"
            aria-label={gt("Card stage workspace tabs")}
            onKeyDown={handleSubTabListKeyDown}
            className="flex flex-wrap items-center gap-1 border-b pb-2"
          >
            <Button
              id="card-workspace-tab-spec"
              role="tab"
              type="button"
              size="xs"
              variant={activeSubTab === "spec" ? "secondary" : "ghost"}
              aria-selected={activeSubTab === "spec"}
              aria-controls="card-workspace-tabpanel-spec"
              tabIndex={activeSubTab === "spec" ? 0 : -1}
              onClick={() => setSelectedSubTab({ stage: card.stage, tab: "spec" })}
            >
              <T>Spec</T>
            </Button>
            <Button
              id="card-workspace-tab-plan"
              role="tab"
              type="button"
              size="xs"
              variant={activeSubTab === "plan" ? "secondary" : "ghost"}
              aria-selected={activeSubTab === "plan"}
              aria-controls="card-workspace-tabpanel-plan"
              tabIndex={activeSubTab === "plan" ? 0 : -1}
              onClick={() => setSelectedSubTab({ stage: card.stage, tab: "plan" })}
            >
              <T>Plan</T>
            </Button>
            <Button
              id="card-workspace-tab-execute"
              role="tab"
              type="button"
              size="xs"
              variant={activeSubTab === "execute" ? "secondary" : "ghost"}
              aria-selected={activeSubTab === "execute"}
              aria-controls="card-workspace-tabpanel-execute"
              tabIndex={activeSubTab === "execute" ? 0 : -1}
              onClick={() => setSelectedSubTab({ stage: card.stage, tab: "execute" })}
            >
              <T>Execute</T>
            </Button>
            <Button
              id="card-workspace-tab-ship"
              role="tab"
              type="button"
              size="xs"
              variant={activeSubTab === "ship" ? "secondary" : "ghost"}
              aria-selected={activeSubTab === "ship"}
              aria-controls="card-workspace-tabpanel-ship"
              tabIndex={activeSubTab === "ship" ? 0 : -1}
              onClick={() => setSelectedSubTab({ stage: card.stage, tab: "ship" })}
            >
              <T>Ship</T>
            </Button>
            <Button
              id="card-workspace-tab-history"
              role="tab"
              type="button"
              size="xs"
              variant={activeSubTab === "history" ? "secondary" : "ghost"}
              aria-selected={activeSubTab === "history"}
              aria-controls="card-workspace-tabpanel-history"
              tabIndex={activeSubTab === "history" ? 0 : -1}
              onClick={() => setSelectedSubTab({ stage: card.stage, tab: "history" })}
            >
              <T>
                History (<Var>{revisions.length}</Var>)
              </T>
            </Button>
          </div>

          {actionError !== null ? (
            <Alert variant="error">
              <TriangleAlertIcon />
              <AlertTitle>
                <T>Action failed</T>
              </AlertTitle>
              <AlertDescription>{actionError}</AlertDescription>
            </Alert>
          ) : null}

          {actionNotice !== null ? (
            <Alert variant="success" role="status">
              <CheckCircle2Icon />
              <AlertDescription>{actionNotice}</AlertDescription>
            </Alert>
          ) : null}

          {invalidation.specStale ? (
            <Alert variant="warning" role="region" aria-labelledby={specStaleTitleId}>
              <TriangleAlertIcon />
              <AlertTitle id={specStaleTitleId}>
                <T>Spec approval is stale</T>
              </AlertTitle>
              <AlertDescription>
                <T>
                  The spec changed after approval. Review the updated spec digest and re-approve
                  before starting Plan.
                </T>
              </AlertDescription>
            </Alert>
          ) : null}

          {invalidation.planStale ? (
            <Alert variant="warning" role="region" aria-labelledby={planStaleTitleId}>
              <TriangleAlertIcon />
              <AlertTitle id={planStaleTitleId}>
                <T>Plan approval is stale</T>
              </AlertTitle>
              <AlertDescription>
                <T>
                  Downstream plan approval was invalidated by a spec or revision change. Review and
                  re-approve the plan before execution.
                </T>
              </AlertDescription>
            </Alert>
          ) : null}

          {invalidation.externalEditNotice ? (
            <Alert variant="warning" role="region" aria-labelledby={externalEditTitleId}>
              <TriangleAlertIcon />
              <AlertTitle id={externalEditTitleId}>
                <T>External edits detected</T>
              </AlertTitle>
              <AlertDescription>{card.blockerSummary}</AlertDescription>
            </Alert>
          ) : null}

          {card.cliImport !== undefined ? (
            <details
              data-cli-import-provenance="true"
              className="rounded-lg border bg-card px-3.5 py-2.5 text-xs"
            >
              <summary className="flex cursor-pointer flex-wrap items-center justify-between gap-2 rounded-sm font-medium text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
                <span>
                  <T>
                    CLI import provenance (<Var>{card.cliImport.sourceSpecId}</Var>)
                  </T>
                </span>
                <span className="font-mono text-2xs text-muted-foreground">
                  <T>
                    <Var>{card.cliImport.decisionIds.length}</Var> decisions ·{" "}
                    <Var>{card.cliImport.assumptionIds.length}</Var> assumptions ·{" "}
                    <Var>{card.cliImport.acceptanceCriterionIds.length}</Var> ACs ·{" "}
                    <Var>{card.cliImport.historicalReceipts.length}</Var> historical receipts
                  </T>
                </span>
              </summary>
              <div className="mt-3 flex flex-col gap-3 border-t pt-3">
                <div className="grid gap-2 sm:grid-cols-3">
                  <div>
                    <span className="text-muted-foreground">
                      <T>Source directory</T>
                    </span>
                    <p className="font-mono text-foreground">{card.cliImport.sourceCliDir}</p>
                  </div>
                  <div>
                    <span className="text-muted-foreground">
                      <T>Source spec ID</T>
                    </span>
                    <p className="font-mono text-foreground">{card.cliImport.sourceSpecId}</p>
                  </div>
                  <div>
                    <span className="text-muted-foreground">
                      <T>Ownership</T>
                    </span>
                    <p className="font-mono text-foreground">{card.cliImport.owner}</p>
                  </div>
                </div>

                {card.cliImport.assurancePolicySummary ? (
                  <p className="text-muted-foreground">{card.cliImport.assurancePolicySummary}</p>
                ) : null}

                {card.cliImport.historicalReceipts.length > 0 ? (
                  <div className="flex flex-col gap-1.5">
                    <span className="font-medium text-foreground">
                      <T>Historical CLI receipts (native revalidation required)</T>
                    </span>
                    <ul className="flex flex-col gap-1">
                      {card.cliImport.historicalReceipts.map((receipt) => (
                        <li
                          key={receipt.id}
                          className="flex flex-wrap items-center justify-between gap-2 rounded border bg-background px-2.5 py-1.5"
                        >
                          <div className="flex flex-wrap items-center gap-1.5">
                            <Badge variant="outline" size="sm">
                              {workflowBoardCliImportHistoricalReceiptKindLabel(
                                receipt.kind,
                                messages,
                              )}
                            </Badge>
                            <Badge variant="secondary" size="sm">
                              <T>Historical</T>
                            </Badge>
                            <span className="text-foreground">{receipt.summary}</span>
                          </div>
                          <span className="font-mono text-2xs text-muted-foreground">
                            {receipt.sourcePath}
                          </span>
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}

                {card.cliImport.unsupportedPolicies.length > 0 ? (
                  <div className="flex flex-col gap-1.5">
                    <span className="font-medium text-foreground">
                      <T>Adopted CLI policies</T>
                    </span>
                    <ul className="flex flex-col gap-1">
                      {card.cliImport.unsupportedPolicies.map((policy) => (
                        <li
                          key={policy.key}
                          className="flex flex-wrap items-center justify-between gap-2 rounded border bg-background px-2.5 py-1.5"
                        >
                          <div className="flex flex-wrap items-center gap-1.5">
                            <span className="font-mono font-medium text-foreground">
                              {policy.key}
                            </span>
                            <Badge
                              variant={policy.status === "acknowledged" ? "success" : "warning"}
                              size="sm"
                            >
                              {workflowBoardCliImportPolicyStatusLabel(policy.status, messages)}
                            </Badge>
                          </div>
                          <span className="text-muted-foreground">{policy.summary}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}

                {card.cliImport.missingLogNotes.length > 0 ? (
                  <div className="flex flex-col gap-1.5">
                    <span className="font-medium text-foreground">
                      <T>Missing CLI check logs</T>
                    </span>
                    <ul className="flex flex-col gap-1">
                      {card.cliImport.missingLogNotes.map((note) => (
                        <li
                          key={`${note.taskId ?? ""}:${note.relativePath}`}
                          className="rounded border bg-background px-2.5 py-1.5 text-muted-foreground"
                        >
                          <span className="font-mono text-foreground">{note.relativePath}</span>:{" "}
                          {note.note}
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}
              </div>
            </details>
          ) : null}

          <div
            role="tabpanel"
            id={`card-workspace-tabpanel-${activeSubTab}`}
            aria-labelledby={`card-workspace-tab-${activeSubTab}`}
            className="flex flex-col gap-5"
          >
            {activeSubTab === "spec" ? (
              <div
                data-workspace-section="spec"
                tabIndex={-1}
                className="flex flex-col gap-5 rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <WorkflowBoardSpecEditor
                  environmentId={props.environmentId}
                  card={card}
                  providers={props.providers}
                  settings={props.settings}
                  modelSelection={effectiveModelSelection}
                  runtimeMode={runtimeMode}
                  interactionMode={interactionMode}
                  workspaceStrategy={workspaceStrategy}
                  workspaceStrategyValid={workspaceStrategyValid}
                  activeSpecThreadId={activeSpecThreadId}
                  onSpecStarted={(threadId) => {
                    setStartedSpecThreadId(threadId);
                    props.onStageStarted?.(threadId);
                  }}
                  onActionError={setActionError}
                  onActionNotice={setActionNotice}
                />

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
                />

                <WorkflowBoardExplorationWizard
                  environmentId={props.environmentId}
                  card={card}
                  onActionError={setActionError}
                  onActionNotice={setActionNotice}
                />

                <WorkflowBoardWireframePreview
                  environmentId={props.environmentId}
                  card={card}
                  onActionError={setActionError}
                  onActionNotice={setActionNotice}
                />

                <WorkflowBoardChallengePanel
                  environmentId={props.environmentId}
                  card={card}
                  onActionError={setActionError}
                  onActionNotice={setActionNotice}
                />

                <div className="flex flex-wrap items-center justify-between gap-2 border-t pt-4">
                  <span className="text-xs text-muted-foreground">
                    {specApprovalState === "approved" ? (
                      <T>Spec is approved. You can advance to the Plan stage.</T>
                    ) : (
                      <T>
                        Resolve exploration decisions and complete bounded challenge to approve the
                        spec.
                      </T>
                    )}
                  </span>
                  <Button
                    type="button"
                    size="sm"
                    disabled={!canApprove || approvingSpec}
                    onClick={() => void handleApproveSpec()}
                  >
                    <CheckCircle2Icon />
                    {approvingSpec ? (
                      <T>Approving…</T>
                    ) : specApprovalState === "approved" ? (
                      <T>Spec approved</T>
                    ) : (
                      <T>Approve spec</T>
                    )}
                  </Button>
                </div>
              </div>
            ) : activeSubTab === "plan" ? (
              <div
                data-workspace-section="plan"
                tabIndex={-1}
                className="rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <WorkflowBoardPlanEditor
                  environmentId={props.environmentId}
                  workspaceRoot={props.workspaceRoot}
                  card={card}
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
                  workspaceStrategyValid={workspaceStrategyValid}
                  activePlanThreadId={activePlanThreadId}
                  onPlanStarted={(threadId) => {
                    setStartedPlanThreadId(threadId);
                    props.onStageStarted?.(threadId);
                  }}
                  onActionError={setActionError}
                  onActionNotice={setActionNotice}
                />
              </div>
            ) : activeSubTab === "execute" ? (
              <WorkflowBoardExecutePanel
                environmentId={props.environmentId}
                projectId={props.projectId}
                card={card}
                cardDiff={cardDiffQuery.data}
                cardDiffLoading={cardDiffQuery.isPending}
                cardDiffError={cardDiffQuery.error}
                onRefreshDiff={() => cardDiffQuery.refresh()}
                onRefreshCard={() => props.onCardRefresh?.()}
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
                workspaceStrategyValid={workspaceStrategyValid}
                focusedSection={executeSectionFocus?.section ?? null}
                highlightedTargetId={executeSectionFocus?.targetId ?? null}
                onSelectThread={(stage, threadId) => {
                  if (stage === "execute") {
                    setStartedExecuteThreadId(threadId);
                  } else {
                    setStartedReviewThreadId(threadId);
                  }
                  setSelectedThreadOverride({ stage, threadId });
                }}
                onStageStarted={(threadId) => props.onStageStarted?.(threadId)}
                {...(props.onRequestTransition !== undefined
                  ? { onRequestTransition: props.onRequestTransition }
                  : {})}
                onActionError={setActionError}
                onActionNotice={setActionNotice}
              />
            ) : activeSubTab === "ship" ? (
              <WorkflowBoardShipPanel
                environmentId={props.environmentId}
                projectId={props.projectId}
                workspaceRoot={props.workspaceRoot}
                card={card}
                cardDiff={cardDiffQuery.data}
                cardDiffLoading={cardDiffQuery.isPending}
                cardDiffError={cardDiffQuery.error}
                displayedThreadId={displayedThreadId}
                highlightedTargetId={shipHighlightTargetId}
                onRefreshDiff={() => cardDiffQuery.refresh()}
                onRefreshCard={() => props.onCardRefresh?.()}
                onNavigateToPointer={handleNavigateToPointer}
                onActionError={setActionError}
                onActionNotice={setActionNotice}
              />
            ) : (
              <section
                aria-labelledby="history-section-heading"
                data-workspace-section="history"
                tabIndex={-1}
                className="flex flex-col gap-3 rounded-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <h3 id="history-section-heading" className="text-sm font-semibold text-foreground">
                  <T>Revision ledger</T>
                </h3>
                {revisions.length === 0 ? (
                  <p className="text-xs text-muted-foreground">
                    <T>No revisions recorded yet.</T>
                  </p>
                ) : (
                  <ol className="flex flex-col gap-2">
                    {revisions.map((entry) => (
                      <li
                        key={`${entry.revision}-${entry.createdAt}`}
                        className="flex flex-col gap-1 rounded-md border bg-card p-3 text-xs"
                      >
                        <div className="flex flex-wrap items-center justify-between gap-2">
                          <div className="flex items-center gap-2">
                            <Badge variant="outline" size="sm">
                              <T>
                                rev <Var>{entry.revision}</Var>
                              </T>
                            </Badge>
                            <Badge variant="secondary" size="sm">
                              {workflowBoardStageLabel(entry.stage, messages)}
                            </Badge>
                            <span className="font-mono text-muted-foreground">{entry.kind}</span>
                          </div>
                          <span className="font-mono text-muted-foreground">{entry.createdAt}</span>
                        </div>
                        <p className="text-foreground">{entry.summary}</p>
                      </li>
                    ))}
                  </ol>
                )}
              </section>
            )}
          </div>
        </div>
      </div>

      {displayedThreadId !== null ? (
        <aside
          {...workspaceAreaProps("board-thread-pane", 3, gt("Connected stage thread"))}
          className="flex min-h-0 w-full shrink-0 flex-col border-t bg-background lg:w-1/2 lg:max-w-2xl lg:border-t-0 lg:border-s"
        >
          <div className="flex h-10 shrink-0 items-center justify-between border-b px-3">
            <span className="text-xs font-medium text-foreground">
              {displayedThreadKind === "spec" ? (
                <T>Connected Spec thread</T>
              ) : displayedThreadKind === "execute" ? (
                <T>Connected Execute thread</T>
              ) : displayedThreadKind === "review" ? (
                <T>Connected Review thread</T>
              ) : (
                <T>Connected Plan thread</T>
              )}
            </span>
            <span className="font-mono text-xs text-muted-foreground">{displayedThreadId}</span>
          </div>
          <WorkflowThreadPane
            key={`${props.environmentId}:${displayedThreadId}`}
            threadRef={scopeThreadRef(props.environmentId, displayedThreadId)}
          />
        </aside>
      ) : null}
    </div>
  );
}

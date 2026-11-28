import { useAtomValue } from "@effect/atom-react";
import { scopeProjectRef } from "@legioncode/client-runtime/environment";
import {
  WORKFLOW_BOARD_STAGES,
  groupWorkflowBoardCards,
  isWorkflowBoardCardShipped,
  workflowBoardAvailabilityNotice,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardProjectNotFoundNotice,
  workflowBoardReadPermissionNotice,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
} from "@legioncode/client-runtime/workflow-board";
import {
  DEFAULT_SERVER_SETTINGS,
  WorkflowCardId,
  type ThreadId,
  type WorkflowBoardCardSummary,
  type WorkflowBoardStage,
} from "@legioncode/contracts";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { T, useGT, useMessages } from "gt-react";
import { FolderInputIcon, KanbanIcon, PlusIcon, WorkflowIcon } from "lucide-react";
import { useState, type KeyboardEvent, type ReactNode } from "react";

import { isElectron } from "~/env";
import { cn } from "../../lib/utils";
import { workspaceAreaProps } from "../../lib/workspaceFocus";
import {
  useAllEnvironmentProjectSnapshotsReady,
  useServerConfigs,
  useThreadShellsForProjectRefs,
} from "../../state/entities";
import {
  useConnectedEnvironmentIds,
  useEnvironmentProviders,
  usePrimaryEnvironmentId,
} from "../../state/environments";
import { useEnvironmentQuery } from "../../state/query";
import { workflowBoard } from "../../state/workflowBoard";
import { useSettingsProjectGroups } from "../settings/useSettingsProjectGroups";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "../ui/empty";
import { Select, SelectItem, SelectPopup, SelectTrigger, SelectValue } from "../ui/select";
import { SidebarInset } from "../ui/sidebar";
import {
  WorkspaceBreadcrumb,
  WorkspaceBreadcrumbItem,
  WorkspaceBreadcrumbSeparator,
  WorkspaceBreadcrumbText,
} from "../WorkspaceBreadcrumb";
import { WorkspacePageHeader } from "../WorkspacePageHeader";
import { WorkflowBoardCardWorkspace } from "./WorkflowBoardCardWorkspace";
import { WorkflowBoardCliImportDialog } from "./WorkflowBoardCliImportDialog";
import { WorkflowBoardCreateCardDialog } from "./WorkflowBoardCreateCardDialog";
import { WorkflowBoardOverview } from "./WorkflowBoardOverview";
import {
  WorkflowBoardTransitionDialog,
  type WorkflowBoardPendingTransition,
} from "./WorkflowBoardTransitionDialog";
import {
  resolveWorkflowBoardProjectTarget,
  type WorkflowBoardSearch,
  type WorkflowBoardTab,
} from "./workflowBoard.logic";

function BoardNotice(props: { readonly title: string; readonly description: ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center p-6">
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <KanbanIcon />
          </EmptyMedia>
          <EmptyTitle>{props.title}</EmptyTitle>
          <EmptyDescription>{props.description}</EmptyDescription>
        </EmptyHeader>
      </Empty>
    </div>
  );
}

function StageSidebarCardButton(props: {
  readonly card: WorkflowBoardCardSummary;
  readonly selected: boolean;
  readonly onSelect: (card: WorkflowBoardCardSummary) => void;
}) {
  const messages = useMessages();
  const blocker = workflowBoardCardBlocker(props.card, messages);
  const nextAction = workflowBoardCardNextAction(props.card, messages);
  const approvalState = workflowBoardSpecApprovalState(props.card);
  const approvalVariant =
    approvalState === "approved" ? "success" : approvalState === "stale" ? "warning" : "secondary";
  const planState = workflowBoardPlanApprovalState(props.card);
  const planVariant =
    planState === "approved" ? "success" : planState === "stale" ? "warning" : "secondary";

  return (
    <button
      type="button"
      onClick={() => props.onSelect(props.card)}
      aria-pressed={props.selected}
      className={cn(
        "flex w-full cursor-pointer flex-col gap-2 rounded-lg border bg-card p-3 text-left transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        props.selected ? "border-primary ring-1 ring-primary" : "border-border",
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <span className="line-clamp-2 text-sm font-medium text-foreground">{props.card.title}</span>
        <Badge variant={props.card.status === "blocked" ? "error" : "outline"} size="sm">
          {workflowBoardCardStatusLabel(props.card.status, messages)}
        </Badge>
      </div>
      {blocker !== null ? (
        <p className="line-clamp-2 text-xs text-destructive-foreground">{blocker}</p>
      ) : props.card.currentTask !== null ? (
        <p className="line-clamp-2 text-xs text-muted-foreground">{props.card.currentTask}</p>
      ) : null}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-xs font-medium text-muted-foreground">{nextAction}</span>
        <div className="flex flex-wrap items-center gap-1">
          {isWorkflowBoardCardShipped(props.card) ? (
            <Badge variant="success" size="sm">
              {workflowBoardDeliveryStatusLabel("shipped", messages)}
            </Badge>
          ) : null}
          <Badge variant={approvalVariant} size="sm">
            {workflowBoardSpecApprovalLabel(props.card.specApprovalStatus, messages)}
          </Badge>
          {props.card.planApprovalStatus !== undefined && props.card.planApprovalStatus !== null ? (
            <Badge variant={planVariant} size="sm">
              {workflowBoardPlanApprovalLabel(props.card.planApprovalStatus, messages)}
            </Badge>
          ) : null}
        </div>
      </div>
    </button>
  );
}

const BOARD_TABS: readonly WorkflowBoardTab[] = ["overview", ...WORKFLOW_BOARD_STAGES];

export function WorkflowBoardPage() {
  const gt = useGT();
  const messages = useMessages();
  const navigate = useNavigate();
  const { projectKey } = useParams({ from: "/_chat/projects/$projectKey/board" });
  const search = useSearch({ from: "/_chat/projects/$projectKey/board" });

  const projectGroups = useSettingsProjectGroups();
  const allProjectSnapshotsReady = useAllEnvironmentProjectSnapshotsReady();
  const serverConfigs = useServerConfigs();
  const connectedEnvironmentIds = useConnectedEnvironmentIds();
  const primaryEnvironmentId = usePrimaryEnvironmentId();
  const providersByEnvironment = useEnvironmentProviders();

  const target = resolveWorkflowBoardProjectTarget({
    projectGroups,
    projectKey,
    requestedEnvironmentId: search.env,
    requestedProjectId: search.projectId,
    serverConfigs,
    connectedEnvironmentIds,
    primaryEnvironmentId,
  });

  const activeEnvironmentId = target.environmentId;
  const activeProjectId = target.projectId;
  const activeMember = target.member;
  const canReadBoard = useAtomValue(workflowBoard.readPermissionAtom(activeEnvironmentId));
  const canCreateCard = useAtomValue(workflowBoard.createCard.permissionAtom(activeEnvironmentId));

  const boardEnabled =
    target.availability === "enabled" && activeEnvironmentId !== null && activeProjectId !== null;

  const snapshotQuery = useEnvironmentQuery(
    boardEnabled && canReadBoard
      ? workflowBoard.boardSnapshot({
          environmentId: activeEnvironmentId,
          input: { projectId: activeProjectId },
        })
      : null,
  );

  const selectedCardId =
    search.card !== undefined && search.card.trim() !== ""
      ? WorkflowCardId.make(search.card.trim())
      : null;

  const cardDetailQuery = useEnvironmentQuery(
    boardEnabled && canReadBoard && selectedCardId !== null
      ? workflowBoard.cardDetail({
          environmentId: activeEnvironmentId,
          input: { cardId: selectedCardId, projectId: activeProjectId },
        })
      : null,
  );

  const projectThreads = useThreadShellsForProjectRefs(
    activeEnvironmentId !== null && activeProjectId !== null
      ? [scopeProjectRef(activeEnvironmentId, activeProjectId)]
      : [],
  );
  const existingWorktreePaths = [
    ...new Set(
      projectThreads.flatMap((thread) =>
        thread.worktreePath !== null && thread.worktreePath.trim() !== ""
          ? [thread.worktreePath]
          : [],
      ),
    ),
  ];

  const [createOpen, setCreateOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [shipSidebarFilter, setShipSidebarFilter] = useState<"all" | "active" | "shipped">("all");
  const [inspectShipNonce, setInspectShipNonce] = useState(0);
  const [pendingTransition, setPendingTransition] = useState<WorkflowBoardPendingTransition | null>(
    null,
  );

  const activeTab: WorkflowBoardTab = search.stage ?? "overview";

  const updateSearch = (next: WorkflowBoardSearch) => {
    void navigate({
      to: "/projects/$projectKey/board",
      params: { projectKey },
      search: next,
      replace: true,
    });
  };

  const selectTab = (tab: WorkflowBoardTab) => {
    updateSearch({
      ...(activeEnvironmentId !== null ? { env: activeEnvironmentId } : {}),
      ...(activeProjectId !== null ? { projectId: activeProjectId } : {}),
      ...(selectedCardId !== null ? { card: selectedCardId } : {}),
      ...(tab === "overview" ? {} : { stage: tab }),
    });
  };

  const handleTabListKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const currentIndex = BOARD_TABS.indexOf(activeTab);
    let nextTab: WorkflowBoardTab | undefined;
    if (event.key === "ArrowRight") {
      nextTab = BOARD_TABS[(currentIndex + 1) % BOARD_TABS.length];
    } else if (event.key === "ArrowLeft") {
      nextTab = BOARD_TABS[(currentIndex - 1 + BOARD_TABS.length) % BOARD_TABS.length];
    } else if (event.key === "Home") {
      nextTab = BOARD_TABS[0];
    } else if (event.key === "End") {
      nextTab = BOARD_TABS[BOARD_TABS.length - 1];
    }
    if (nextTab === undefined) return;
    event.preventDefault();
    selectTab(nextTab);
    document.getElementById(`workflow-board-tab-${nextTab}`)?.focus();
  };

  const selectCard = (cardId: WorkflowCardId, stage?: WorkflowBoardStage) => {
    const nextStage = stage ?? search.stage;
    updateSearch({
      ...(activeEnvironmentId !== null ? { env: activeEnvironmentId } : {}),
      ...(activeProjectId !== null ? { projectId: activeProjectId } : {}),
      card: cardId,
      ...(nextStage === undefined ? {} : { stage: nextStage }),
    });
  };

  const availabilityNotice = workflowBoardAvailabilityNotice(target.availability, messages);
  const projectNotFoundNotice = workflowBoardProjectNotFoundNotice(messages);
  const readPermissionNotice = workflowBoardReadPermissionNotice(messages);

  const allCards = snapshotQuery.data?.cards ?? [];
  const columns = groupWorkflowBoardCards(allCards);
  const activeStageCards =
    activeTab === "overview"
      ? []
      : activeTab === "ship"
        ? shipSidebarFilter === "active"
          ? columns.ship
          : shipSidebarFilter === "shipped"
            ? columns.completed
            : [...columns.ship, ...columns.completed]
        : columns[activeTab];
  const providers =
    activeEnvironmentId !== null ? (providersByEnvironment.get(activeEnvironmentId) ?? []) : [];
  const serverSettings =
    (activeEnvironmentId !== null ? serverConfigs.get(activeEnvironmentId)?.settings : undefined) ??
    DEFAULT_SERVER_SETTINGS;

  return (
    <SidebarInset className="h-dvh min-h-0 overflow-hidden overscroll-y-none">
      <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-background">
        <WorkspacePageHeader electron={isElectron} className="border-b">
          <WorkspaceBreadcrumb ariaLabel={gt("Workflow board breadcrumb")} className="flex-1">
            <WorkspaceBreadcrumbItem>
              <Link
                to="/projects/$projectKey"
                params={{ projectKey }}
                className="inline-flex min-w-0 max-w-full cursor-pointer items-center rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <WorkspaceBreadcrumbText>
                  {target.group?.displayName ?? projectKey}
                </WorkspaceBreadcrumbText>
              </Link>
            </WorkspaceBreadcrumbItem>
            <WorkspaceBreadcrumbSeparator>
              <WorkspaceBreadcrumbText>/</WorkspaceBreadcrumbText>
            </WorkspaceBreadcrumbSeparator>
            <WorkspaceBreadcrumbItem current>
              <h1 className="truncate">
                <T>Workflow board</T>
              </h1>
            </WorkspaceBreadcrumbItem>
          </WorkspaceBreadcrumb>

          {target.candidates.length > 1 && activeMember !== null ? (
            <Select
              value={`${activeMember.environmentId}:${activeMember.id}`}
              onValueChange={(value) => {
                if (typeof value !== "string") return;
                const picked = target.candidates.find(
                  (candidate) => `${candidate.environmentId}:${candidate.id}` === value,
                );
                if (picked === undefined) return;
                setCreateOpen(false);
                setImportOpen(false);
                setPendingTransition(null);
                setShipSidebarFilter("all");
                updateSearch({
                  env: picked.environmentId,
                  projectId: picked.id,
                  ...(search.stage !== undefined ? { stage: search.stage } : {}),
                });
              }}
            >
              <SelectTrigger size="sm" aria-label={gt("Checkout")}>
                <SelectValue>
                  {activeMember.environmentLabel ?? activeMember.environmentId}
                </SelectValue>
              </SelectTrigger>
              <SelectPopup>
                {target.candidates.map((candidate) => (
                  <SelectItem
                    key={`${candidate.environmentId}:${candidate.id}`}
                    value={`${candidate.environmentId}:${candidate.id}`}
                  >
                    {candidate.environmentLabel ?? candidate.environmentId}
                  </SelectItem>
                ))}
              </SelectPopup>
            </Select>
          ) : null}

          <Button size="sm" variant="outline" render={<Link to="/workflows" />}>
            <WorkflowIcon />
            <T>Open workflows</T>
          </Button>

          {boardEnabled ? (
            <>
              <Button
                size="sm"
                variant="outline"
                disabled={!canCreateCard}
                onClick={() => setImportOpen(true)}
              >
                <FolderInputIcon />
                <T>Import CLI project</T>
              </Button>
              <Button size="sm" disabled={!canCreateCard} onClick={() => setCreateOpen(true)}>
                <PlusIcon />
                <T>New card</T>
              </Button>
            </>
          ) : null}
        </WorkspacePageHeader>

        {target.group === null && !allProjectSnapshotsReady ? (
          <p className="p-6 text-sm text-muted-foreground" role="status">
            <T>Loading workflow board…</T>
          </p>
        ) : target.group === null ||
          activeMember === null ||
          activeEnvironmentId === null ||
          activeProjectId === null ? (
          <BoardNotice
            title={projectNotFoundNotice.title}
            description={projectNotFoundNotice.description}
          />
        ) : target.availability === "loading" ? (
          <p className="p-6 text-sm text-muted-foreground" role="status">
            <T>Loading workflow board…</T>
          </p>
        ) : availabilityNotice !== null ? (
          <BoardNotice
            title={availabilityNotice.title}
            description={
              availabilityNotice.action === "open-labs-settings" ? (
                <T>
                  Enable Workflows in{" "}
                  <Link
                    to="/settings/labs"
                    className="rounded-sm underline underline-offset-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                  >
                    Settings → Labs
                  </Link>{" "}
                  to use the project workflow board.
                </T>
              ) : (
                availabilityNotice.description
              )
            }
          />
        ) : !canReadBoard ? (
          <BoardNotice
            title={readPermissionNotice.title}
            description={readPermissionNotice.description}
          />
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">
            <div
              role="tablist"
              aria-label={gt("Workflow board stages")}
              onKeyDown={handleTabListKeyDown}
              className="flex shrink-0 items-center gap-1 border-b px-4 py-2"
            >
              <Button
                id="workflow-board-tab-overview"
                role="tab"
                type="button"
                size="xs"
                variant={activeTab === "overview" ? "secondary" : "ghost"}
                aria-selected={activeTab === "overview"}
                aria-controls="workflow-board-tabpanel-overview"
                tabIndex={activeTab === "overview" ? 0 : -1}
                onClick={() => selectTab("overview")}
              >
                <T>Overview</T>
              </Button>
              {WORKFLOW_BOARD_STAGES.map((stage) => (
                <Button
                  key={stage}
                  id={`workflow-board-tab-${stage}`}
                  role="tab"
                  type="button"
                  size="xs"
                  variant={activeTab === stage ? "secondary" : "ghost"}
                  aria-selected={activeTab === stage}
                  aria-controls={`workflow-board-tabpanel-${stage}`}
                  tabIndex={activeTab === stage ? 0 : -1}
                  onClick={() => selectTab(stage)}
                >
                  {workflowBoardStageLabel(stage, messages)} ({columns[stage].length})
                </Button>
              ))}
            </div>

            <div
              role="tabpanel"
              id={`workflow-board-tabpanel-${activeTab}`}
              aria-labelledby={`workflow-board-tab-${activeTab}`}
              className="flex min-h-0 flex-1 flex-col"
            >
              {snapshotQuery.error !== null ? (
                <BoardNotice
                  title={gt("Could not load workflow board")}
                  description={snapshotQuery.error}
                />
              ) : snapshotQuery.data === null ? (
                <p className="p-6 text-sm text-muted-foreground" role="status">
                  <T>Loading workflow board…</T>
                </p>
              ) : activeTab === "overview" ? (
                <WorkflowBoardOverview
                  columns={columns}
                  allCards={allCards}
                  selectedCardId={selectedCardId}
                  canOperate={canCreateCard}
                  onSelectTab={selectTab}
                  onSelectCard={(picked) => selectCard(picked.cardId, picked.stage)}
                  onRequestTransition={(card, transition) =>
                    setPendingTransition({ card, transition })
                  }
                />
              ) : (
                <div className="flex min-h-0 flex-1 flex-col md:flex-row">
                  <div
                    {...workspaceAreaProps(
                      "board-stage-list",
                      1,
                      workflowBoardStageLabel(activeTab, messages),
                    )}
                    className="flex min-h-0 w-full shrink-0 flex-col gap-2 overflow-y-auto border-b p-3 md:w-72 md:border-e md:border-b-0"
                  >
                    <div className="flex items-center justify-between px-1">
                      <h2 className="text-xs font-semibold uppercase text-muted-foreground">
                        {workflowBoardStageLabel(activeTab, messages)}
                      </h2>
                      <Badge variant="outline" size="sm">
                        {activeStageCards.length}
                      </Badge>
                    </div>
                    {activeTab === "ship" &&
                    (columns.ship.length > 0 || columns.completed.length > 0) ? (
                      <div
                        role="group"
                        aria-label={gt("Filter Ship cards")}
                        className="flex flex-wrap items-center gap-1 px-1"
                      >
                        <Button
                          type="button"
                          size="xs"
                          variant={shipSidebarFilter === "all" ? "secondary" : "ghost"}
                          aria-pressed={shipSidebarFilter === "all"}
                          onClick={() => setShipSidebarFilter("all")}
                        >
                          <T>All</T>
                        </Button>
                        <Button
                          type="button"
                          size="xs"
                          variant={shipSidebarFilter === "active" ? "secondary" : "ghost"}
                          aria-pressed={shipSidebarFilter === "active"}
                          onClick={() => setShipSidebarFilter("active")}
                        >
                          <T>In Ship</T>
                        </Button>
                        <Button
                          type="button"
                          size="xs"
                          variant={shipSidebarFilter === "shipped" ? "secondary" : "ghost"}
                          aria-pressed={shipSidebarFilter === "shipped"}
                          onClick={() => setShipSidebarFilter("shipped")}
                        >
                          <T>Shipped</T>
                        </Button>
                      </div>
                    ) : null}
                    {activeStageCards.length === 0 ? (
                      <div className="flex flex-col items-start gap-2 rounded-lg border border-dashed p-3">
                        <p className="text-xs text-muted-foreground">
                          <T>No cards in this stage.</T>
                        </p>
                        {activeTab === "spec" ? (
                          <div className="flex flex-wrap items-center gap-2">
                            <Button
                              size="xs"
                              variant="outline"
                              disabled={!canCreateCard}
                              onClick={() => setCreateOpen(true)}
                            >
                              <PlusIcon />
                              <T>New card</T>
                            </Button>
                            <Button
                              size="xs"
                              variant="outline"
                              disabled={!canCreateCard}
                              onClick={() => setImportOpen(true)}
                            >
                              <FolderInputIcon />
                              <T>Import CLI project</T>
                            </Button>
                          </div>
                        ) : null}
                      </div>
                    ) : (
                      activeStageCards.map((card) => (
                        <StageSidebarCardButton
                          key={card.cardId}
                          card={card}
                          selected={selectedCardId === card.cardId}
                          onSelect={(picked) => selectCard(picked.cardId, activeTab)}
                        />
                      ))
                    )}
                  </div>

                  <div className="flex min-h-0 min-w-0 flex-1 flex-col">
                    {selectedCardId === null ? (
                      <BoardNotice
                        title={gt("Select a card")}
                        description={gt("Pick a card from this stage or create a new card.")}
                      />
                    ) : cardDetailQuery.error !== null ? (
                      <BoardNotice
                        title={gt("Could not load card")}
                        description={cardDetailQuery.error}
                      />
                    ) : cardDetailQuery.data === null ? (
                      <p className="p-6 text-sm text-muted-foreground" role="status">
                        <T>Loading card…</T>
                      </p>
                    ) : (
                      <WorkflowBoardCardWorkspace
                        key={`${activeEnvironmentId}:${activeProjectId}:${cardDetailQuery.data.cardId}`}
                        environmentId={activeEnvironmentId}
                        projectId={activeProjectId}
                        workspaceRoot={activeMember.workspaceRoot}
                        card={cardDetailQuery.data}
                        requestedStage={search.stage}
                        inspectShipNonce={inspectShipNonce}
                        providers={providers}
                        settings={serverSettings}
                        existingWorktreePaths={existingWorktreePaths}
                        onStageStarted={(_threadId: ThreadId) => {
                          cardDetailQuery.refresh();
                        }}
                        onCardRefresh={() => {
                          cardDetailQuery.refresh();
                        }}
                        onRequestTransition={(card, transition) =>
                          setPendingTransition({ card, transition })
                        }
                      />
                    )}
                  </div>
                </div>
              )}
            </div>
          </div>
        )}
      </div>

      {boardEnabled && createOpen && activeEnvironmentId !== null && activeProjectId !== null ? (
        <WorkflowBoardCreateCardDialog
          open={createOpen}
          onOpenChange={setCreateOpen}
          environmentId={activeEnvironmentId}
          projectId={activeProjectId}
          providers={providers}
          settings={serverSettings}
          existingWorktreePaths={existingWorktreePaths}
          onCreated={(cardId) => selectCard(cardId, "spec")}
        />
      ) : null}

      {boardEnabled && importOpen && activeEnvironmentId !== null && activeProjectId !== null ? (
        <WorkflowBoardCliImportDialog
          open={importOpen}
          onOpenChange={setImportOpen}
          environmentId={activeEnvironmentId}
          projectId={activeProjectId}
          providers={providers}
          settings={serverSettings}
          existingWorktreePaths={existingWorktreePaths}
          onImported={(cardId, stage) => selectCard(cardId, stage)}
        />
      ) : null}

      {boardEnabled && activeEnvironmentId !== null ? (
        <WorkflowBoardTransitionDialog
          pending={pendingTransition}
          environmentId={activeEnvironmentId}
          onClose={() => setPendingTransition(null)}
          onCompleted={(nextStage) => {
            if (pendingTransition !== null) {
              selectCard(pendingTransition.card.cardId, nextStage);
            }
            cardDetailQuery.refresh();
          }}
          onInspectShip={(cardId) => {
            setPendingTransition(null);
            setInspectShipNonce((n) => n + 1);
            selectCard(cardId, "ship");
          }}
        />
      ) : null}
    </SidebarInset>
  );
}

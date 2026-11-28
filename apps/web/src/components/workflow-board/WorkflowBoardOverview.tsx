import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  pointerWithin,
  rectIntersection,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DragEndEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  WORKFLOW_BOARD_STAGES,
  isWorkflowBoardCardShipped,
  workflowBoardCardBlocker,
  workflowBoardCardNextAction,
  workflowBoardCardStatusLabel,
  workflowBoardDeliveryStatusLabel,
  workflowBoardPlanApprovalLabel,
  workflowBoardPlanApprovalState,
  workflowBoardSpecApprovalLabel,
  workflowBoardSpecApprovalState,
  workflowBoardStageLabel,
  type WorkflowBoardColumns,
} from "@legioncode/client-runtime/workflow-board";
import type { WorkflowBoardCardSummary, WorkflowBoardStage } from "@legioncode/contracts";
import { T, useGT, useMessages, Var } from "gt-react";
import { ArrowLeftIcon, ArrowRightIcon, GripVerticalIcon } from "lucide-react";
import { useState, type KeyboardEvent } from "react";

import { isEditableFocused } from "../../lib/editableFocus";
import { cn } from "../../lib/utils";
import { workspaceAreaProps } from "../../lib/workspaceFocus";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import {
  resolveCrossColumnTransition,
  type WorkflowBoardCrossColumnTransition,
} from "./workflowBoard.logic";

function columnDroppableId(stage: WorkflowBoardStage): string {
  return `board-col:${stage}`;
}

function parseColumnDroppableId(id: string): WorkflowBoardStage | null {
  if (!id.startsWith("board-col:")) return null;
  const stage = id.slice("board-col:".length);
  if (stage === "spec" || stage === "plan" || stage === "execute" || stage === "ship") {
    return stage;
  }
  return null;
}

function CardSummaryBody(props: {
  readonly card: WorkflowBoardCardSummary;
  readonly selected: boolean;
}) {
  const messages = useMessages();
  const blocker = workflowBoardCardBlocker(props.card, messages);
  const nextAction = workflowBoardCardNextAction(props.card, messages);
  const specState = workflowBoardSpecApprovalState(props.card);
  const specVariant =
    specState === "approved" ? "success" : specState === "stale" ? "warning" : "secondary";
  const planState = workflowBoardPlanApprovalState(props.card);
  const planVariant =
    planState === "approved" ? "success" : planState === "stale" ? "warning" : "secondary";

  return (
    <>
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
      <div className="flex flex-wrap items-center justify-between gap-1.5">
        <span className="text-xs font-medium text-muted-foreground">{nextAction}</span>
        <div className="flex flex-wrap items-center gap-1">
          {isWorkflowBoardCardShipped(props.card) ? (
            <Badge variant="success" size="sm">
              {workflowBoardDeliveryStatusLabel("shipped", messages)}
            </Badge>
          ) : null}
          <Badge variant={specVariant} size="sm">
            {workflowBoardSpecApprovalLabel(props.card.specApprovalStatus, messages)}
          </Badge>
          {props.card.planApprovalStatus !== undefined && props.card.planApprovalStatus !== null ? (
            <Badge variant={planVariant} size="sm">
              {workflowBoardPlanApprovalLabel(props.card.planApprovalStatus, messages)}
            </Badge>
          ) : null}
        </div>
      </div>
    </>
  );
}

function DraggableOverviewCard(props: {
  readonly card: WorkflowBoardCardSummary;
  readonly selected: boolean;
  readonly canOperate: boolean;
  readonly onSelect: (card: WorkflowBoardCardSummary) => void;
  readonly onRequestTransition: (
    card: WorkflowBoardCardSummary,
    transition: WorkflowBoardCrossColumnTransition,
  ) => void;
  readonly onMoveFocus?: (direction: -1 | 1) => void;
}) {
  const gt = useGT();
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: props.card.cardId,
    data: { card: props.card },
    disabled: !props.canOperate || props.card.status === "completed",
  });

  const forwardStage: WorkflowBoardStage | null =
    props.card.stage === "spec"
      ? "plan"
      : props.card.stage === "plan"
        ? "execute"
        : props.card.stage === "execute"
          ? "ship"
          : null;
  const forwardTransition =
    forwardStage !== null ? resolveCrossColumnTransition(props.card, forwardStage) : null;

  const backwardStage: WorkflowBoardStage | null =
    props.card.stage === "plan"
      ? "spec"
      : props.card.stage === "execute"
        ? "plan"
        : props.card.stage === "ship"
          ? "execute"
          : null;
  const backwardTransition =
    backwardStage !== null ? resolveCrossColumnTransition(props.card, backwardStage) : null;

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (isEditableFocused(event.target)) return;
    if (event.key === "ArrowRight" && event.altKey && forwardTransition !== null) {
      event.preventDefault();
      props.onRequestTransition(props.card, forwardTransition);
      return;
    }
    if (event.key === "ArrowLeft" && event.altKey && backwardTransition !== null) {
      event.preventDefault();
      props.onRequestTransition(props.card, backwardTransition);
      return;
    }
    if (
      event.key === "ArrowDown" &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      props.onMoveFocus?.(1);
      return;
    }
    if (
      event.key === "ArrowUp" &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      props.onMoveFocus?.(-1);
    }
  };

  const keyShortcuts = props.canOperate
    ? [
        forwardTransition !== null ? "Alt+ArrowRight" : null,
        backwardTransition !== null ? "Alt+ArrowLeft" : null,
      ]
        .filter((shortcut): shortcut is string => shortcut !== null)
        .join(" ")
    : "";

  return (
    <div
      ref={setNodeRef}
      data-board-card-id={props.card.cardId}
      onKeyDown={handleKeyDown}
      className={cn(
        "flex flex-col gap-2 rounded-lg border bg-card p-3 transition-colors hover:bg-accent/40",
        props.selected ? "border-primary ring-1 ring-primary" : "border-border",
        isDragging ? "opacity-50" : undefined,
      )}
    >
      <div className="flex items-start gap-1.5">
        <button
          type="button"
          data-board-drag-handle={props.card.cardId}
          aria-label={gt("Drag {title} to another stage", { title: props.card.title })}
          disabled={!props.canOperate || props.card.status === "completed"}
          className="mt-0.5 shrink-0 cursor-grab rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring disabled:cursor-default disabled:opacity-40"
          {...attributes}
          {...listeners}
        >
          <GripVerticalIcon className="size-3.5" />
        </button>
        <button
          type="button"
          onClick={() => props.onSelect(props.card)}
          aria-pressed={props.selected}
          aria-keyshortcuts={keyShortcuts !== "" ? keyShortcuts : undefined}
          onPointerDown={(event) => event.stopPropagation()}
          className="flex min-w-0 flex-1 cursor-pointer flex-col gap-2 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        >
          <CardSummaryBody card={props.card} selected={props.selected} />
        </button>
      </div>

      {forwardTransition !== null || backwardTransition !== null ? (
        <div
          className="flex flex-wrap items-center justify-end gap-1 border-t pt-2"
          onPointerDown={(event) => event.stopPropagation()}
        >
          {backwardTransition !== null ? (
            <Button
              type="button"
              size="xs"
              variant="ghost"
              disabled={!props.canOperate}
              aria-label={gt("Return {title} for revision", { title: props.card.title })}
              onClick={() => props.onRequestTransition(props.card, backwardTransition)}
            >
              <ArrowLeftIcon />
              <T>Return</T>
            </Button>
          ) : null}
          {forwardTransition !== null ? (
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={!props.canOperate}
              aria-label={
                forwardTransition.kind === "approve_spec"
                  ? gt("Move {title} to Plan", { title: props.card.title })
                  : forwardTransition.kind === "approve_plan"
                    ? gt("Move {title} to Execute", { title: props.card.title })
                    : gt("Move {title} to Ship", { title: props.card.title })
              }
              onClick={() => props.onRequestTransition(props.card, forwardTransition)}
            >
              {forwardTransition.kind === "approve_spec" ? (
                <T>Approve spec</T>
              ) : forwardTransition.kind === "approve_plan" ? (
                <T>Approve plan</T>
              ) : (
                <T>Approve delivery</T>
              )}
              <ArrowRightIcon />
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function StageColumn(props: {
  readonly stage: WorkflowBoardStage;
  readonly order: number;
  readonly cards: ReadonlyArray<WorkflowBoardCardSummary>;
  readonly selectedCardId: string | null;
  readonly canOperate: boolean;
  readonly activeDragCard: WorkflowBoardCardSummary | null;
  readonly onSelectTab: (stage: WorkflowBoardStage) => void;
  readonly onSelectCard: (card: WorkflowBoardCardSummary) => void;
  readonly onRequestTransition: (
    card: WorkflowBoardCardSummary,
    transition: WorkflowBoardCrossColumnTransition,
  ) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const stageLabel = workflowBoardStageLabel(props.stage, messages);
  const { setNodeRef, isOver } = useDroppable({
    id: columnDroppableId(props.stage),
  });
  const canDrop =
    props.activeDragCard !== null &&
    resolveCrossColumnTransition(props.activeDragCard, props.stage) !== null;

  const focusAdjacentCard = (currentCardId: string, direction: -1 | 1) => {
    const idx = props.cards.findIndex((c) => c.cardId === currentCardId);
    const next = props.cards[idx + direction];
    if (next === undefined) return;
    const node = document.querySelector<HTMLElement>(
      `[data-board-card-id="${next.cardId}"] button:not([data-board-drag-handle])`,
    );
    node?.focus();
  };

  return (
    <section
      ref={setNodeRef}
      {...workspaceAreaProps(`board-col-${props.stage}`, props.order, stageLabel)}
      data-board-stage-column={props.stage}
      className={cn(
        "flex min-h-48 flex-col gap-2 rounded-xl border p-3 transition-colors",
        isOver && canDrop ? "border-primary bg-primary/5" : "bg-muted/20",
      )}
    >
      <h2>
        <button
          type="button"
          onClick={() => props.onSelectTab(props.stage)}
          aria-label={gt("Open {stage} stage ({count} cards)", {
            stage: stageLabel,
            count: props.cards.length,
          })}
          className="flex w-full cursor-pointer items-center justify-between rounded-md px-1 py-0.5 text-left hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        >
          <span className="text-sm font-semibold text-foreground">{stageLabel}</span>
          <Badge variant="outline" size="sm">
            {props.cards.length}
          </Badge>
        </button>
      </h2>
      <div className="flex flex-col gap-2">
        {props.cards.length === 0 ? (
          <p className="px-1 py-4 text-xs text-muted-foreground">
            <T>No cards in this stage.</T>
          </p>
        ) : (
          props.cards.map((card) => (
            <DraggableOverviewCard
              key={card.cardId}
              card={card}
              selected={props.selectedCardId === card.cardId}
              canOperate={props.canOperate}
              onSelect={props.onSelectCard}
              onRequestTransition={props.onRequestTransition}
              onMoveFocus={(dir) => focusAdjacentCard(card.cardId, dir)}
            />
          ))
        )}
      </div>
    </section>
  );
}

export function WorkflowBoardOverview(props: {
  readonly columns: WorkflowBoardColumns;
  readonly allCards: ReadonlyArray<WorkflowBoardCardSummary>;
  readonly selectedCardId: string | null;
  readonly canOperate: boolean;
  readonly onSelectTab: (stage: WorkflowBoardStage) => void;
  readonly onSelectCard: (card: WorkflowBoardCardSummary) => void;
  readonly onRequestTransition: (
    card: WorkflowBoardCardSummary,
    transition: WorkflowBoardCrossColumnTransition,
  ) => void;
}) {
  const gt = useGT();
  const messages = useMessages();
  const [activeCardId, setActiveCardId] = useState<string | null>(null);
  const [shippedOnly, setShippedOnly] = useState(false);

  const pointerSensorOptions = { activationConstraint: { distance: 6 } };
  const sensors = useSensors(
    useSensor(PointerSensor, pointerSensorOptions),
    useSensor(KeyboardSensor),
  );

  const activeDragCard =
    activeCardId === null
      ? null
      : (props.allCards.find((candidate) => candidate.cardId === activeCardId) ?? null);

  const collisionDetection: CollisionDetection = (args) => {
    const collisions =
      args.pointerCoordinates !== null ? pointerWithin(args) : rectIntersection(args);
    const draggedCard = props.allCards.find((candidate) => candidate.cardId === args.active.id);
    if (!draggedCard) return collisions;
    return collisions.filter((collision) => {
      const stage = parseColumnDroppableId(String(collision.id));
      return stage !== null && resolveCrossColumnTransition(draggedCard, stage) !== null;
    });
  };

  const handleDragStart = (event: DragStartEvent) => {
    setActiveCardId(String(event.active.id));
  };

  const handleDragEnd = (event: DragEndEvent) => {
    const draggedId = String(event.active.id);
    const overId = event.over?.id;
    setActiveCardId(null);
    if (overId === undefined) return;
    const targetStage = parseColumnDroppableId(String(overId));
    if (targetStage === null) return;
    const draggedCard = props.allCards.find((candidate) => candidate.cardId === draggedId);
    if (!draggedCard) return;
    const transition = resolveCrossColumnTransition(draggedCard, targetStage);
    if (transition !== null) {
      props.onRequestTransition(draggedCard, transition);
    }
  };

  const resolveCardTitle = (id: string | number) =>
    props.allCards.find((candidate) => candidate.cardId === String(id))?.title ?? String(id);
  const resolveOverStageLabel = (overId: string | number | undefined) => {
    if (overId === undefined) return null;
    const stage = parseColumnDroppableId(String(overId));
    return stage !== null ? workflowBoardStageLabel(stage, messages) : null;
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto p-4">
      <DndContext
        sensors={sensors}
        collisionDetection={collisionDetection}
        accessibility={{
          screenReaderInstructions: {
            draggable: gt(
              "To pick up a workflow card, press Space or Enter. Use the arrow keys to move to a target stage column, and press Space or Enter again to drop, or Escape to cancel.",
            ),
          },
          announcements: {
            onDragStart({ active }) {
              return gt("Picked up card {title}.", { title: resolveCardTitle(active.id) });
            },
            onDragOver({ active, over }) {
              const title = resolveCardTitle(active.id);
              const overStage = resolveOverStageLabel(over?.id);
              return overStage !== null
                ? gt("Card {title} is over the {stage} column.", { title, stage: overStage })
                : gt("Card {title} is not over an available transition column.", { title });
            },
            onDragEnd({ active, over }) {
              const title = resolveCardTitle(active.id);
              const overStage = resolveOverStageLabel(over?.id);
              return overStage !== null
                ? gt("Dropped card {title} onto the {stage} column.", {
                    title,
                    stage: overStage,
                  })
                : gt("Dropped card {title} without moving stages.", { title });
            },
            onDragCancel({ active }) {
              return gt("Cancelled moving card {title}.", {
                title: resolveCardTitle(active.id),
              });
            },
          },
        }}
        onDragStart={handleDragStart}
        onDragEnd={handleDragEnd}
        onDragCancel={() => setActiveCardId(null)}
      >
        <div className="grid min-h-0 flex-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
          {WORKFLOW_BOARD_STAGES.map((stage, index) => (
            <StageColumn
              key={stage}
              stage={stage}
              order={index + 1}
              cards={props.columns[stage]}
              selectedCardId={props.selectedCardId}
              canOperate={props.canOperate}
              activeDragCard={activeDragCard}
              onSelectTab={props.onSelectTab}
              onSelectCard={props.onSelectCard}
              onRequestTransition={props.onRequestTransition}
            />
          ))}
        </div>

        <DragOverlay>
          {activeDragCard !== null ? (
            <div className="flex flex-col gap-2 rounded-lg border border-primary bg-card p-3 shadow-lg">
              <CardSummaryBody card={activeDragCard} selected />
            </div>
          ) : null}
        </DragOverlay>
      </DndContext>

      {props.columns.completed.length > 0 ? (
        <section
          aria-label={gt("Completed cards")}
          className="mt-4 flex flex-col gap-2 rounded-xl border bg-muted/20 p-3"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-sm font-semibold text-foreground">
              <T>
                Completed (<Var>{props.columns.completed.length}</Var>)
              </T>
            </h2>
            <Button
              type="button"
              size="xs"
              variant={shippedOnly ? "secondary" : "ghost"}
              aria-pressed={shippedOnly}
              onClick={() => setShippedOnly((prev) => !prev)}
            >
              <T>Shipped only</T>
            </Button>
          </div>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {props.columns.completed
              .filter((card) => !shippedOnly || isWorkflowBoardCardShipped(card))
              .map((card) => (
                <button
                  key={card.cardId}
                  type="button"
                  onClick={() => props.onSelectCard(card)}
                  aria-pressed={props.selectedCardId === card.cardId}
                  className={cn(
                    "flex w-full cursor-pointer flex-col gap-2 rounded-lg border bg-card p-3 text-left transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                    props.selectedCardId === card.cardId
                      ? "border-primary ring-1 ring-primary"
                      : "border-border",
                  )}
                >
                  <CardSummaryBody card={card} selected={props.selectedCardId === card.cardId} />
                </button>
              ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}

import {
  closestCorners,
  DndContext,
  getFirstCollision,
  KeyboardCode,
  KeyboardSensor,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
  type DragMoveEvent,
  type DragOverEvent,
  type KeyboardCoordinateGetter,
} from "@dnd-kit/core";
import { CSS } from "@dnd-kit/utilities";
import { createMemoryHistory } from "@tanstack/react-router";
import {
  PositiveInt,
  ThreadId,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WS_METHODS,
  type WorkflowBoardApprovalReceipt,
  type WorkflowBoardApprovalStatus,
  type WorkflowBoardCardStatus,
  type WorkflowBoardDeliveryStatus,
  type WorkflowBoardStage,
  type WorkflowCardDetail,
} from "@legioncode/contracts";
import { DEFAULT_CLIENT_SETTINGS } from "@legioncode/contracts/settings";
import type * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as Stream from "effect/Stream";
import { describe, expect, it, vi } from "vite-plus/test";
import { page, userEvent } from "vite-plus/test/browser";

import { AppRoot } from "~/AppRoot";
import { openCommandPalette } from "~/commandPaletteBus";
import { useCommandPaletteStore } from "~/commandPaletteStore";
import "~/components/ChatView";
import { __setClientSettingsForTests } from "~/hooks/useSettings";
import "~/index.css";
import { getRouter } from "~/router";
import { appAtomRegistry } from "~/rpc/atomRegistry";
import {
  ENVIRONMENT_ID,
  PROJECT_ID,
  answerRpc,
  installFakeEnvironment,
  threadWithHistory,
  type FakeDriver,
  type FakeThread,
} from "../../../../test/fakeEnvironment";
import { act, render } from "../../../../test/render";
import {
  dispatchPointerDrag,
  hasProjectWorkflowBoardCapability,
  makeBoardServerConfigFixture,
  renderWithBoardRouter,
  withDesktopViewport,
} from "./workflowBoardE2eHarness";
import {
  BOARD_AT,
  boardRequestError,
  installWorkflowBoardFixtures,
  makeBoardCardDetail,
} from "./workflowBoardE2eFixtures";

const boardEnvState = vi.hoisted(() => ({
  workflowsEnabled: false,
  projectWorkflowBoard: false,
  configListeners: new Set<() => void>(),
  emitConfigSnapshot() {
    for (const listener of this.configListeners) {
      listener();
    }
  },
}));

vi.mock(
  "../../../../../../packages/client-runtime/src/connection/driver.ts",
  async (importOriginal) => {
    const [actual, { fakeDriver }, Layer, EffectMod, QueueMod, StreamMod] = await Promise.all([
      importOriginal<{ readonly ConnectionDriver: Context.Key<unknown, FakeDriver> }>(),
      import("../../../../test/fakeEnvironment"),
      import("effect/Layer"),
      import("effect/Effect"),
      import("effect/Queue"),
      import("effect/Stream"),
    ]);

    const buildAugmentedConfig = (base: Record<string, unknown>) => {
      const env = (base.environment ?? {}) as Record<string, unknown>;
      const caps = (env.capabilities ?? {}) as Record<string, unknown>;
      const settings = (base.settings ?? {}) as Record<string, unknown>;
      return {
        ...base,
        environment: {
          ...env,
          capabilities: {
            ...caps,
            projectWorkflowBoard: boardEnvState.projectWorkflowBoard,
          },
        },
        settings: {
          ...settings,
          workflowsEnabled: boardEnvState.workflowsEnabled,
        },
      };
    };

    const wrappedDriver: FakeDriver = {
      ...fakeDriver,
      connect: (entry) =>
        EffectMod.map(fakeDriver.connect(entry), (lease) => ({
          ...lease,
          session: {
            ...lease.session,
            initialConfig: EffectMod.map(lease.session.initialConfig, (cfg) =>
              buildAugmentedConfig(cfg as unknown as Record<string, unknown>),
            ) as never,
            subscribeServerConfig: (() =>
              StreamMod.unwrap(
                EffectMod.gen(function* () {
                  const baseCfg = yield* lease.session.initialConfig;
                  const queue = yield* QueueMod.unbounded<unknown>();
                  const offerSnapshot = () =>
                    QueueMod.offerUnsafe(queue, {
                      version: 1 as const,
                      type: "snapshot" as const,
                      config: buildAugmentedConfig(baseCfg as unknown as Record<string, unknown>),
                    });
                  const listener = () => {
                    void offerSnapshot();
                  };
                  offerSnapshot();
                  boardEnvState.configListeners.add(listener);
                  yield* EffectMod.addFinalizer(() =>
                    EffectMod.sync(() => {
                      boardEnvState.configListeners.delete(listener);
                    }),
                  );
                  return StreamMod.fromQueue(queue);
                }),
              )) as never,
          },
        })),
    };

    return { ...actual, layer: Layer.succeed(actual.ConnectionDriver, wrappedDriver) };
  },
);

vi.mock(import("~/rpc/requestLatencyState"), async (original) => ({
  ...(await original()),
  trackRpcRequestSent: () => {},
}));

type BoardStage = WorkflowBoardStage;

interface BoardCardFixture {
  readonly detail: WorkflowCardDetail;
  readonly artifacts: Readonly<Record<string, string>>;
}

interface BoardCardFixtureOptions {
  readonly cardId: string;
  readonly title: string;
  readonly stage?: BoardStage;
  readonly status?: WorkflowBoardCardStatus;
  readonly blocker?: string | null;
  readonly currentTask?: string | null;
  readonly nextAction?: string | null;
  readonly revision?: number;
  readonly threadId?: string | null;
  readonly specApprovalStatus?: WorkflowBoardApprovalStatus | null;
  readonly planApprovalStatus?: WorkflowBoardApprovalStatus | null;
  readonly deliveryStatus?: WorkflowBoardDeliveryStatus | null;
  readonly artifacts?: Readonly<Record<string, string>>;
}

function makeBoardCardFixture(options: BoardCardFixtureOptions): BoardCardFixture {
  const revision = options.revision ?? 1;
  const artifacts = options.artifacts ?? {};
  return {
    artifacts,
    detail: makeBoardCardDetail({
      cardId: options.cardId,
      title: options.title,
      stage: options.stage ?? "spec",
      status: options.status ?? (options.blocker ? "blocked" : "active"),
      revision: PositiveInt.make(revision),
      currentTask:
        options.currentTask === undefined ? "Drafting specification" : options.currentTask,
      blockerSummary: options.blocker ?? null,
      nextAction: options.nextAction === undefined ? "draft_spec" : options.nextAction,
      activeThreadId: options.threadId ? ThreadId.make(options.threadId) : null,
      specApprovalStatus: options.specApprovalStatus ?? null,
      specApprovedRevision: options.specApprovalStatus ? PositiveInt.make(revision) : null,
      ...(options.planApprovalStatus !== undefined
        ? {
            planApprovalStatus: options.planApprovalStatus,
            planApprovedRevision: options.planApprovalStatus ? PositiveInt.make(revision) : null,
          }
        : {}),
      ...(options.deliveryStatus !== undefined ? { deliveryStatus: options.deliveryStatus } : {}),
      artifactKeys: ["spec.md", ...Object.keys(artifacts)].map((key) =>
        WorkflowBoardArtifactKey.make(key),
      ),
    }),
  };
}

function makeApprovalReceipt(
  gate: "spec" | "plan",
  card: WorkflowCardDetail,
  revision: number,
): WorkflowBoardApprovalReceipt {
  return {
    id: WorkflowBoardApprovalId.make(`approval-${gate}-${card.cardId}-${revision}`),
    gate,
    status: "valid",
    revision: PositiveInt.make(revision),
    artifactDigest: `sha256:${gate}-${card.cardId}-${revision}`,
    specDigest: card.spec.specDigest,
    wireframesDigest: null,
    approvedAt: BOARD_AT,
    approvedBy: "board-e2e-operator",
  };
}

function installBoardRpcFixtures(initialCards: ReadonlyArray<BoardCardFixture>) {
  const store = installWorkflowBoardFixtures(initialCards.map((card) => card.detail));
  const loadArtifacts = (cards: ReadonlyArray<BoardCardFixture>) => {
    for (const card of cards) {
      for (const [key, content] of Object.entries(card.artifacts)) {
        store.artifacts.set(`${card.detail.cardId}:${key}`, content);
      }
    }
  };
  loadArtifacts(initialCards);

  const assertRevision = (card: WorkflowCardDetail, input: Record<string, unknown>) => {
    if (card.revision !== input.expectedRevision) {
      throw boardRequestError(
        "revision_conflict",
        `Revision mismatch: expected ${card.revision}, got ${String(input.expectedRevision)}`,
      );
    }
  };

  store.on(WS_METHODS.workflowBoardApproveSpec, (input, s) => {
    const card = s.get(String(input.cardId));
    assertRevision(card, input);
    const specApproval = makeApprovalReceipt("spec", card, card.revision + 1);
    const next = s.advance(card.cardId, {
      stage: "plan",
      specApproval,
      specApprovalStatus: "valid",
      specApprovedRevision: PositiveInt.make(card.revision + 1),
      nextAction: "start_plan",
    });
    return { cardId: next.cardId, revision: next.revision, specApproval, card: next };
  });

  store.on(WS_METHODS.workflowBoardReturnForRevision, (input, s) => {
    const card = s.get(String(input.cardId));
    assertRevision(card, input);
    const targetStage = input.targetStage as BoardStage;
    const history = [
      ...(card.history ?? card.revisions),
      {
        revision: PositiveInt.make(card.revision + 1),
        stage: targetStage,
        kind: "return_for_revision",
        summary: String(input.reason),
        specDigest: card.spec.specDigest,
        createdAt: BOARD_AT,
      },
    ];
    const next = s.advance(card.cardId, {
      stage: targetStage,
      planApprovalStatus: "stale",
      nextAction: targetStage === "spec" ? "revise_spec" : "revise_plan",
      history,
      revisions: history,
    });
    return {
      cardId: next.cardId,
      revision: next.revision,
      stage: next.stage,
      specApproval: next.specApproval,
      planApproval: next.planApproval ?? null,
      history,
      card: next,
    };
  });

  return {
    store,
    stub: {
      calls: store.calls,
      defer: (method: string) => store.hold(method),
    },
    pushSnapshot(nextCards?: ReadonlyArray<BoardCardFixture>) {
      if (nextCards) {
        store.cards.clear();
        for (const card of nextCards) store.cards.set(card.detail.cardId, card.detail);
        loadArtifacts(nextCards);
      }
      store.pushSnapshot();
    },
    [Symbol.dispose]() {
      store[Symbol.dispose]();
    },
  };
}

function setupBoardAppEnvironment(
  options: {
    readonly workflowsEnabled: boolean;
    readonly projectWorkflowBoard: boolean;
  },
  threads: ReadonlyArray<FakeThread> = [threadWithHistory("board-nav-thread", 1)],
) {
  useCommandPaletteStore.setState(useCommandPaletteStore.getInitialState(), true);
  boardEnvState.workflowsEnabled = options.workflowsEnabled;
  boardEnvState.projectWorkflowBoard = options.projectWorkflowBoard;
  boardEnvState.emitConfigSnapshot?.();
  __setClientSettingsForTests({
    ...DEFAULT_CLIENT_SETTINGS,
    boringSidebarEnabled: true,
    onboardingCompletedAt: "2026-01-01T00:00:00.000Z",
  });
  return installFakeEnvironment(threads);
}

async function mountBoardAppAt(initialEntry: string, width = 1360, height = 860) {
  await using stack = new AsyncDisposableStack();
  appAtomRegistry.reset();
  stack.use(await withDesktopViewport(width, height));
  const router = getRouter(createMemoryHistory({ initialEntries: [initialEntry] }));
  await router.load();
  const screen = stack.use(
    await render(<AppRoot router={router} />, {
      // AppRoot owns its router instance.
      router: false,
    }),
  );
  const owned = stack.move();
  return {
    ...screen,
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}

type BoardRpcFixtures = ReturnType<typeof installBoardRpcFixtures>;
const BOARD_COLUMNS = ["Spec", "Plan", "Execute", "Ship"] as const;
type BoardColumnLabel = (typeof BOARD_COLUMNS)[number];

function stageColumnHeading(stage: BoardColumnLabel) {
  return page.getByRole("heading", { name: new RegExp(`^Open ${stage} stage \\(\\d+ cards?\\)$`) });
}

async function expectBoardColumnsRendered(rpc: BoardRpcFixtures) {
  await expect
    .element(page.getByRole("heading", { name: "Workflow board", level: 1 }))
    .toBeVisible();
  for (const column of BOARD_COLUMNS) {
    await expect.element(stageColumnHeading(column)).toBeVisible();
  }
  await vi.waitFor(() => {
    expect(
      rpc.stub.calls.some(
        (call) =>
          call.method === WS_METHODS.workflowBoardSubscribe && call.input.projectId === PROJECT_ID,
      ),
    ).toBe(true);
  });
}

async function openBoardViaCommandPalette(rpc: BoardRpcFixtures) {
  openCommandPalette();
  const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
  await expect.element(paletteInput).toBeVisible();
  await userEvent.fill(paletteInput, "workflow board");
  const option = page.getByRole("option", { name: /open workflow board/i });
  await expect.element(option).toBeVisible();
  await userEvent.click(option);
  await expectBoardColumnsRendered(rpc);
}

async function openBoardViaSidebar(rpc: BoardRpcFixtures) {
  const projects = page.getByRole("combobox", { name: "Projects" });
  await expect.element(projects).toBeVisible();
  const trigger = page.getByRole("button", { name: "Workflow board for render" });
  await expect.element(trigger).toBeVisible();
  await userEvent.click(trigger);
  await expectBoardColumnsRendered(rpc);
}

function findColumnElement(stage: BoardColumnLabel): Element {
  const column = document.querySelector(`[data-board-stage-column="${stage.toLowerCase()}"]`);
  if (!column) throw new Error(`Board column ${stage} is not rendered`);
  return column;
}

function findCardElement(cardId: string): Element {
  const card = document.querySelector(`[data-board-card-id="${cardId}"]`);
  if (!card) throw new Error(`Board card ${cardId} is not rendered`);
  return card;
}

function findCardDragHandle(cardId: string): Element {
  const handle = document.querySelector(`[data-board-drag-handle="${cardId}"]`);
  if (!handle) throw new Error(`Board card ${cardId} has no drag handle`);
  return handle;
}

function cardSelectButton(title: string) {
  return page
    .getByRole("button", { name: new RegExp(`^${title}`), pressed: false })
    .or(page.getByRole("button", { name: new RegExp(`^${title}`), pressed: true }));
}
async function dispatchPointerCancelDrag(sourceEl: Element, targetEl: Element): Promise<void> {
  const srcRect = sourceEl.getBoundingClientRect();
  const dstRect = targetEl.getBoundingClientRect();
  const srcX = srcRect.left + srcRect.width / 2;
  const srcY = srcRect.top + srcRect.height / 2;
  const dstX = dstRect.left + dstRect.width / 2;
  const dstY = dstRect.top + dstRect.height / 2;
  const dx = dstX - srcX;
  const dy = dstY - srcY;
  const dist = Math.hypot(dx, dy);
  const moveX = dist > 0 ? srcX + (dx / dist) * 20 : srcX + 20;
  const moveY = dist > 0 ? srcY + (dy / dist) * 20 : srcY;

  await act(async () => {
    sourceEl.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        isPrimary: true,
        button: 0,
        buttons: 1,
        clientX: srcX,
        clientY: srcY,
      }),
    );
  });
  await act(async () => {
    document.dispatchEvent(
      new PointerEvent("pointermove", {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        isPrimary: true,
        button: 0,
        buttons: 1,
        clientX: moveX,
        clientY: moveY,
      }),
    );
  });
  await act(async () => {
    document.dispatchEvent(
      new PointerEvent("pointercancel", {
        bubbles: true,
        cancelable: true,
        pointerId: 1,
        isPrimary: true,
        button: 0,
        buttons: 0,
        clientX: moveX,
        clientY: moveY,
      }),
    );
  });
}

const boardColumnKeyboardCoordinates: KeyboardCoordinateGetter = (
  event,
  { context: { active, collisionRect, droppableRects, droppableContainers } },
) => {
  if (!active || !collisionRect) return undefined;
  if (
    event.code !== KeyboardCode.Right &&
    event.code !== KeyboardCode.Left &&
    event.code !== KeyboardCode.Down &&
    event.code !== KeyboardCode.Up
  ) {
    return undefined;
  }
  event.preventDefault();
  const activeCenterX = collisionRect.left + collisionRect.width / 2;
  const activeCenterY = collisionRect.top + collisionRect.height / 2;
  const candidates = droppableContainers.getEnabled().filter((entry) => {
    if (!entry || entry.disabled) return false;
    const rect = droppableRects.get(entry.id);
    if (!rect) return false;
    const rectCenterX = rect.left + rect.width / 2;
    const rectCenterY = rect.top + rect.height / 2;
    if (event.code === KeyboardCode.Right) return rectCenterX > activeCenterX + 1;
    if (event.code === KeyboardCode.Left) return rectCenterX < activeCenterX - 1;
    if (event.code === KeyboardCode.Down) return rectCenterY > activeCenterY + 1;
    return rectCenterY < activeCenterY - 1;
  });
  const collisions = closestCorners({
    active,
    collisionRect,
    droppableRects,
    droppableContainers: candidates,
    pointerCoordinates: null,
  });
  const targetId = getFirstCollision(collisions, "id");
  const targetRect = targetId != null ? droppableRects.get(targetId) : null;
  return targetRect ? { x: targetRect.left + 8, y: targetRect.top + 8 } : undefined;
};

function BoardStageDraggable({
  id,
  onTriggerButton,
}: {
  readonly id: string;
  readonly onTriggerButton: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform } = useDraggable({ id });
  return (
    <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
      <div
        ref={setNodeRef}
        data-testid={`board-card-${id}`}
        aria-label={`Drag ${id}`}
        style={{
          width: 96,
          height: 36,
          ...(transform ? { transform: CSS.Translate.toString(transform) } : {}),
        }}
        {...attributes}
        {...listeners}
      >
        {id}
      </div>
      <button type="button" onClick={onTriggerButton}>
        Move {id} to Plan
      </button>
    </div>
  );
}

function BoardStageColumn({ id, label }: { readonly id: string; readonly label: string }) {
  const { setNodeRef } = useDroppable({ id });
  return (
    <section
      ref={setNodeRef}
      data-testid={`board-col-${id}`}
      aria-label={label}
      style={{ width: 140, height: 96 }}
    >
      <h3>{label}</h3>
    </section>
  );
}

function BoardDndParityHarness({
  onOpenTransitionModal,
  onDragStart,
  onDragOver,
  onDragMove,
}: {
  readonly onOpenTransitionModal: (sourceId: string, targetCol: string, via: string) => void;
  readonly onDragStart?: (activeId: string) => void;
  readonly onDragOver?: (activeId: string, overId: string | null) => void;
  readonly onDragMove?: (activeId: string, overId: string | null) => void;
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: boardColumnKeyboardCoordinates }),
  );
  const handleDragOver = (event: DragOverEvent) => {
    onDragOver?.(String(event.active.id), event.over ? String(event.over.id) : null);
  };
  const handleDragMove = (event: DragMoveEvent) => {
    onDragMove?.(String(event.active.id), event.over ? String(event.over.id) : null);
  };
  const handleDragEnd = (event: DragEndEvent) => {
    if (event.over) {
      const via = event.activatorEvent instanceof KeyboardEvent ? "keyboard" : "dnd";
      onOpenTransitionModal(String(event.active.id), String(event.over.id), via);
    }
  };
  return (
    <DndContext
      sensors={sensors}
      onDragStart={(event) => onDragStart?.(String(event.active.id))}
      onDragOver={handleDragOver}
      onDragMove={handleDragMove}
      onDragEnd={handleDragEnd}
    >
      <div style={{ display: "flex", gap: 24, padding: 16 }}>
        <BoardStageColumn id="spec-col" label="Spec" />
        <BoardStageDraggable
          id="card-alpha"
          onTriggerButton={() => onOpenTransitionModal("card-alpha", "plan-col", "button")}
        />
        <BoardStageColumn id="plan-col" label="Plan" />
        <BoardStageColumn id="execute-col" label="Execute" />
        <BoardStageColumn id="ship-col" label="Ship" />
      </div>
    </DndContext>
  );
}

describe("workflowBoardE2eBoardNav (F1–F3)", () => {
  it("[UNCOND-WN-01] toggles workflowsEnabled via LabsSettingsPanel and exposes Open workflows in CommandPalette", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: false,
      projectWorkflowBoard: false,
    });
    const updateCalls: Array<Record<string, unknown>> = [];
    using _updateRpc = answerRpc(
      WS_METHODS.serverUpdateSettings,
      (input: Record<string, unknown>) => {
        updateCalls.push(input);
        const patch = (input.patch ?? input) as { readonly workflowsEnabled?: boolean };
        if (typeof patch.workflowsEnabled === "boolean") {
          boardEnvState.workflowsEnabled = patch.workflowsEnabled;
          boardEnvState.emitConfigSnapshot?.();
        }
        return Effect.succeed({ settings: { workflowsEnabled: boardEnvState.workflowsEnabled } });
      },
    );

    {
      await using _screen = await mountBoardAppAt("/settings/labs");
      const toggle = page.getByRole("switch", { name: "Workflows" });
      await expect.element(toggle).toBeVisible();
      await expect.element(toggle).toHaveAttribute("aria-checked", "false");

      await userEvent.click(toggle);
      await vi.waitFor(() => expect(updateCalls.length).toBeGreaterThan(0));
      expect(boardEnvState.workflowsEnabled).toBe(true);
    }

    await using _homeScreen = await mountBoardAppAt("/");
    openCommandPalette();
    const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
    await expect.element(paletteInput).toBeVisible();
    await userEvent.fill(paletteInput, "Open workflows");
    await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await expect
      .element(page.getByRole("dialog", { name: "Command palette" }))
      .not.toBeInTheDocument();
  });

  it("[UNCOND-WN-02] preserves ChatView and /workflows usability on legacy server with projectWorkflowBoard: false", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    const cfg = makeBoardServerConfigFixture({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    expect(hasProjectWorkflowBoardCapability(cfg)).toBe(false);

    {
      await using _chatScreen = await mountBoardAppAt(`/${ENVIRONMENT_ID}/board-nav-thread`);
      await expect.element(page.getByText("board-nav-thread: question 0")).toBeVisible();
      await expect
        .element(
          page
            .getByTestId("sidebar-project-folder")
            .getByRole("button", { name: "New thread in render" }),
        )
        .toBeVisible();
    }

    using _subWf = answerRpc(WS_METHODS.workflowsSubscribe, () =>
      Stream.make({ workflows: [], runs: [], questions: [] }),
    );
    using _cntWf = answerRpc(WS_METHODS.workflowsCounts, () =>
      Stream.make({ pendingQuestions: 0, running: 0 }),
    );
    await using workflowsScreen = await mountBoardAppAt("/workflows");
    await expect.element(workflowsScreen.getByRole("heading", { name: "Workflows" })).toBeVisible();
    await expect.element(workflowsScreen.getByText("No workflow runs yet")).toBeVisible();
  });

  it("[UNCOND-WN-03] verifies @dnd-kit pointer drag, keyboard drag, button parity, sub-threshold jitter, and Escape cancel", async () => {
    const transitions: Array<{
      readonly sourceId: string;
      readonly targetCol: string;
      readonly via: string;
    }> = [];
    const dragStarts: Array<string> = [];
    const dragOvers: Array<{ readonly active: string; readonly over: string | null }> = [];
    const dragMoves: Array<{ readonly active: string; readonly over: string | null }> = [];
    await using screen = await renderWithBoardRouter(
      <BoardDndParityHarness
        onOpenTransitionModal={(sourceId, targetCol, via) => {
          transitions.push({ sourceId, targetCol, via });
        }}
        onDragStart={(activeId) => {
          dragStarts.push(activeId);
        }}
        onDragOver={(active, over) => {
          dragOvers.push({ active, over });
        }}
        onDragMove={(active, over) => {
          dragMoves.push({ active, over });
        }}
      />,
      { width: 1200, height: 760 },
    );

    const cardLocator = screen.getByTestId("board-card-card-alpha");
    const planLocator = screen.getByTestId("board-col-plan-col");
    await expect.element(cardLocator).toBeVisible();
    await expect.element(planLocator).toBeVisible();

    await userEvent.click(screen.getByRole("button", { name: "Move card-alpha to Plan" }));
    expect(transitions).toEqual([{ sourceId: "card-alpha", targetCol: "plan-col", via: "button" }]);

    const cardEl = cardLocator.element();
    const planEl = planLocator.element();
    {
      using _drag = await dispatchPointerDrag(cardEl, planEl);
    }
    expect(transitions).toHaveLength(2);
    expect(transitions[1]).toEqual({
      sourceId: "card-alpha",
      targetCol: "plan-col",
      via: "dnd",
    });

    // Sub-threshold pointer jitter (< 6px) must not fire a drag transition.
    const rect = cardEl.getBoundingClientRect();
    const startX = rect.left + rect.width / 2;
    const startY = rect.top + rect.height / 2;
    await act(async () => {
      cardEl.dispatchEvent(
        new PointerEvent("pointerdown", {
          bubbles: true,
          cancelable: true,
          pointerId: 1,
          isPrimary: true,
          button: 0,
          buttons: 1,
          clientX: startX,
          clientY: startY,
        }),
      );
    });
    await act(async () => {
      document.dispatchEvent(
        new PointerEvent("pointermove", {
          bubbles: true,
          cancelable: true,
          pointerId: 1,
          isPrimary: true,
          button: 0,
          buttons: 1,
          clientX: startX + 3,
          clientY: startY,
        }),
      );
    });
    await act(async () => {
      document.dispatchEvent(
        new PointerEvent("pointerup", {
          bubbles: true,
          cancelable: true,
          pointerId: 1,
          isPrimary: true,
          button: 0,
          buttons: 0,
          clientX: startX + 3,
          clientY: startY,
        }),
      );
    });
    expect(transitions).toHaveLength(2);

    // Keyboard activation + Escape cancels cleanly without firing transition.
    const startsBeforeEscape = dragStarts.length;
    await userEvent.click(cardLocator);
    await userEvent.keyboard(" ");
    await vi.waitFor(() => expect(dragStarts).toHaveLength(startsBeforeEscape + 1));
    await userEvent.keyboard("{Escape}");
    expect(transitions).toHaveLength(2);

    // Completed keyboard drag: Space -> ArrowRight -> Space drops onto plan-col via keyboard.
    const startsBeforeMove = dragStarts.length;
    const movesBeforeMove = dragMoves.length;
    await userEvent.click(cardLocator);
    await userEvent.keyboard(" ");
    await vi.waitFor(() => expect(dragStarts).toHaveLength(startsBeforeMove + 1));
    await userEvent.keyboard("{ArrowRight}");
    await vi.waitFor(() => expect(dragMoves.length).toBeGreaterThan(movesBeforeMove));
    await vi.waitFor(() =>
      expect(dragOvers).toContainEqual({ active: "card-alpha", over: "plan-col" }),
    );
    await userEvent.keyboard(" ");
    await vi.waitFor(() => expect(transitions).toHaveLength(3));
    expect(transitions[2]).toEqual({
      sourceId: "card-alpha",
      targetCol: "plan-col",
      via: "keyboard",
    });
  });

  it("[F1-T1-04] preserves existing /workflows route as the run inspector even when board capability is enabled", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using _subWf = answerRpc(WS_METHODS.workflowsSubscribe, () =>
      Stream.make({ workflows: [], runs: [], questions: [] }),
    );
    using _cntWf = answerRpc(WS_METHODS.workflowsCounts, () =>
      Stream.make({ pendingQuestions: 0, running: 0 }),
    );

    await using screen = await mountBoardAppAt("/workflows");
    await expect.element(screen.getByRole("heading", { name: "Workflows" })).toBeVisible();
  });

  it("[F1-T2-01] hides board entrypoints when projectWorkflowBoard is false or workflowsEnabled is false", async () => {
    {
      using _envDisabled = setupBoardAppEnvironment({
        workflowsEnabled: false,
        projectWorkflowBoard: true,
      });
      await using _screenDisabled = await mountBoardAppAt("/");
      await expect
        .element(
          page
            .getByTestId("sidebar-project-folder")
            .getByRole("button", { name: "New thread in render" }),
        )
        .toBeVisible();
      expect(page.getByRole("button", { name: /workflow board/i }).elements()).toHaveLength(0);
      expect(page.getByRole("link", { name: /workflow board/i }).elements()).toHaveLength(0);

      openCommandPalette();
      const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
      await expect.element(paletteInput).toBeVisible();
      await userEvent.fill(paletteInput, "workflow board");
      expect(page.getByRole("option", { name: /workflow board/i }).elements()).toHaveLength(0);
      await userEvent.keyboard("{Escape}");
      await expect
        .element(page.getByRole("dialog", { name: "Command palette" }))
        .not.toBeInTheDocument();
    }

    {
      using _envLegacy = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      await using _screenLegacy = await mountBoardAppAt("/");
      boardEnvState.emitConfigSnapshot();
      await expect
        .element(
          page
            .getByTestId("sidebar-project-folder")
            .getByRole("button", { name: "New thread in render" }),
        )
        .toBeVisible();
      expect(page.getByRole("button", { name: /workflow board/i }).elements()).toHaveLength(0);
      expect(page.getByRole("link", { name: /workflow board/i }).elements()).toHaveLength(0);

      openCommandPalette();
      const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
      await expect.element(paletteInput).toBeVisible();
      await userEvent.fill(paletteInput, "Open workflows");
      await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
      await userEvent.fill(paletteInput, "workflow board");
      expect(page.getByRole("option", { name: /workflow board/i }).elements()).toHaveLength(0);
      await userEvent.keyboard("{Escape}");
      await expect
        .element(page.getByRole("dialog", { name: "Command palette" }))
        .not.toBeInTheDocument();
    }
  });

  it("[F1-T1-01] opens Project Workflow Board from project sidebar and command palette with 4 ordered columns", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    const seed = [makeBoardCardFixture({ cardId: "card-f1-01", title: "Alpha Spec" })];

    {
      using rpc = installBoardRpcFixtures(seed);
      await using _screen = await mountBoardAppAt("/");
      await openBoardViaCommandPalette(rpc);

      const lefts = BOARD_COLUMNS.map(
        (stage) => stageColumnHeading(stage).element().getBoundingClientRect().left,
      );
      expect(lefts[0]).toBeLessThan(lefts[1]!);
      expect(lefts[1]).toBeLessThan(lefts[2]!);
      expect(lefts[2]).toBeLessThan(lefts[3]!);
      expect(
        rpc.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.subscribe" &&
            (c.input as { readonly projectId?: string })?.projectId === PROJECT_ID,
        ),
      ).toBe(true);
    }

    {
      using rpc = installBoardRpcFixtures(seed);
      await using _screen = await mountBoardAppAt("/");
      await openBoardViaSidebar(rpc);

      const lefts = BOARD_COLUMNS.map(
        (stage) => stageColumnHeading(stage).element().getBoundingClientRect().left,
      );
      expect(lefts[0]).toBeLessThan(lefts[1]!);
      expect(lefts[1]).toBeLessThan(lefts[2]!);
      expect(lefts[2]).toBeLessThan(lefts[3]!);
    }
  });

  it("[F1-T1-02] renders multiple cards per stage with title, current task or blocker, and single next action", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    const cards = [
      makeBoardCardFixture({
        cardId: "spec-1",
        title: "Spec Card One",
        stage: "spec",
        currentTask: "Answer DEC-1",
        nextAction: "explore_spec",
      }),
      makeBoardCardFixture({
        cardId: "spec-2",
        title: "Spec Card Two",
        stage: "spec",
        currentTask: "Review challenge",
        nextAction: "approve_spec",
      }),
      makeBoardCardFixture({
        cardId: "plan-1",
        title: "Plan Card One",
        stage: "plan",
        currentTask: "Compare alternatives",
        nextAction: "author_plan",
      }),
      makeBoardCardFixture({
        cardId: "plan-2",
        title: "Plan Card Two",
        stage: "plan",

        blocker: "Missing AC-02 mapping",
        currentTask: null,
        nextAction: "revise_plan",
      }),
      makeBoardCardFixture({
        cardId: "exec-1",
        title: "Execute Card One",
        stage: "execute",
        currentTask: "Run TSK-0001",
        nextAction: "ready_for_ship",
      }),
    ];
    using rpc = installBoardRpcFixtures(cards);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    const specCol = findColumnElement("Spec");
    const planCol = findColumnElement("Plan");
    const executeCol = findColumnElement("Execute");

    await expect.element(page.getByText("Spec Card One")).toBeVisible();
    await expect.element(page.getByText("Spec Card Two")).toBeVisible();
    await expect.element(page.getByText("Plan Card One")).toBeVisible();
    await expect.element(page.getByText("Plan Card Two")).toBeVisible();
    await expect.element(page.getByText("Execute Card One")).toBeVisible();

    expect(specCol.textContent).toContain("Spec Card One");
    expect(specCol.textContent).toContain("Spec Card Two");
    expect(planCol.textContent).toContain("Plan Card One");
    expect(planCol.textContent).toContain("Missing AC-02 mapping");
    expect(executeCol.textContent).toContain("Run TSK-0001");

    for (const [cardId, label] of [
      ["spec-1", "Continue guided exploration"],
      ["spec-2", "Approve spec"],
      ["plan-1", "Open Plan thread"],
      ["exec-1", "Review for delivery"],
    ] as const) {
      expect(findCardElement(cardId).textContent).toContain(label);
    }
  });

  it("[F1-T1-03] keeps blocked cards visible in their current column and exposes shipped cards in completed view", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-blocked",
        title: "Blocked Plan Feature",
        stage: "plan",
        status: "blocked",

        blocker: "Awaiting DEC-2 decision",
        nextAction: "Resolve Blocker",
      }),
      makeBoardCardFixture({
        cardId: "card-shipped",
        title: "Shipped feature",
        stage: "ship",
        status: "completed",
        nextAction: "View Receipt",
      }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    const planCol = findColumnElement("Plan");
    expect(planCol.textContent).toContain("Blocked Plan Feature");
    expect(planCol.textContent).toContain("Awaiting DEC-2 decision");

    const completed = page.getByRole("region", { name: "Completed cards" });
    await expect.element(completed.getByText("Shipped feature")).toBeVisible();
    await completed.getByRole("button", { name: "Shipped only" }).click();
    await expect.element(completed.getByText("Shipped feature")).toBeVisible();
  });

  it("[F1-T2-02] renders empty board CTA and truncates extreme-length card title and blocker without viewport overflow", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    await expect.element(page.getByRole("button", { name: "New card" }).first()).toBeVisible();
    expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(window.innerWidth);

    const extremeTitle = "X".repeat(240);
    const extremeBlocker = "Blocker ".repeat(23).trim();
    rpc.pushSnapshot([
      makeBoardCardFixture({
        cardId: "card-extreme",
        title: extremeTitle,
        stage: "spec",

        blocker: extremeBlocker,
      }),
    ]);

    await expect.element(page.getByText(extremeTitle)).toBeVisible();
    expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(window.innerWidth);
    const cardRect = findCardElement("card-extreme").getBoundingClientRect();
    expect(cardRect.right).toBeLessThanOrEqual(window.innerWidth);
  });

  it("[F2-T1-01] selecting a column header switches the active specialized stage tab", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({ cardId: "c-spec", title: "Spec Item", stage: "spec" }),
      makeBoardCardFixture({ cardId: "c-plan", title: "Plan Item", stage: "plan" }),
      makeBoardCardFixture({ cardId: "c-exec", title: "Exec Item", stage: "execute" }),
      makeBoardCardFixture({ cardId: "c-ship", title: "Ship Item", stage: "ship" }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    await stageColumnHeading("Plan").getByRole("button").click();
    await expect
      .element(page.getByRole("tab", { name: /^Plan/i }))
      .toHaveAttribute("aria-selected", "true");
    await expect.element(page.getByRole("tabpanel")).toBeVisible();

    await userEvent.click(page.getByRole("tab", { name: /^Execute/i }));
    await expect
      .element(page.getByRole("tab", { name: /^Execute/i }))
      .toHaveAttribute("aria-selected", "true");
    await expect
      .element(page.getByRole("tab", { name: /^Plan/i }))
      .toHaveAttribute("aria-selected", "false");
  });

  it("[F2-T1-02] selecting a card opens working artifacts alongside embedded ordinary LegionCode ChatView", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true }, [
      threadWithHistory("board-card-thread", 1),
    ]);
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-split",
        title: "Split Pane Card",
        stage: "plan",
        threadId: "board-card-thread",
        artifacts: { "plan-brief.md": "# Spec Artifact\nAC-01: Split pane renders" },
      }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    await userEvent.click(page.getByText("Split Pane Card"));
    await vi.waitFor(() => {
      expect(
        rpc.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.getCard" &&
            (c.input as { readonly cardId?: string })?.cardId === "card-split",
        ),
      ).toBe(true);
    });
    await expect.element(page.getByText("AC-01: Split pane renders")).toBeVisible();
    await expect.element(page.getByText("board-card-thread: question 0")).toBeVisible();
    await expect.element(page.getByText("Answer 0 for board-card-thread.")).toBeVisible();
  });

  it("[F2-T1-03] switching between cards fetches getCard and getArtifact on demand and updates both split panes", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true }, [
      threadWithHistory("thread-a", 1),
      threadWithHistory("thread-b", 1),
    ]);
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-a",
        title: "Card Alpha",
        stage: "plan",
        threadId: "thread-a",
        artifacts: { "plan-brief.md": "Artifact Alpha" },
      }),
      makeBoardCardFixture({
        cardId: "card-b",
        title: "Card Beta",
        stage: "plan",
        threadId: "thread-b",
        artifacts: { "plan-brief.md": "Artifact Beta" },
      }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    expect(rpc.stub.calls.some((c) => c.method === "workflowBoard.getCard")).toBe(false);

    await userEvent.click(page.getByText("Card Alpha"));
    await expect.element(page.getByText("Artifact Alpha")).toBeVisible();
    await expect.element(page.getByText("thread-a: question 0")).toBeVisible();
    expect(
      rpc.stub.calls.some(
        (c) =>
          c.method === "workflowBoard.getCard" &&
          (c.input as { readonly cardId?: string })?.cardId === "card-a",
      ),
    ).toBe(true);

    await userEvent.click(page.getByText("Card Beta"));
    await expect.element(page.getByText("Artifact Beta")).toBeVisible();
    await expect.element(page.getByText("thread-b: question 0")).toBeVisible();
    await expect.element(page.getByText("Artifact Alpha")).not.toBeInTheDocument();
  });

  it("[F2-T2-02] renders working artifacts and Start stage placeholder cleanly when threadId is null", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-no-thread",
        title: "Unstarted Plan Card",
        stage: "plan",
        threadId: null,
        artifacts: { "plan-brief.md": "Plan brief ready" },
      }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    await userEvent.click(page.getByText("Unstarted Plan Card"));
    await expect.element(page.getByText("Plan brief ready")).toBeVisible();
    expect(page.getByRole("alert").elements()).toHaveLength(0);
    await expect.element(page.getByRole("button", { name: "Start Plan" })).toBeVisible();
  });

  it("[F2-T2-03] displays loading indicator while getArtifact is deferred and shows inline retry on failure", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-deferred",
        title: "Deferred Artifact Card",
        stage: "plan",
        artifacts: { "plan-brief.md": "Recovered Artifact" },
      }),
    ]);
    const gate = rpc.stub.defer("workflowBoard.getArtifact");
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    await userEvent.click(page.getByText("Deferred Artifact Card"));
    await expect.element(page.getByText("Loading Plan brief…")).toBeVisible();
    await vi.waitFor(() => {
      expect(rpc.store.callsTo(WS_METHODS.workflowBoardGetArtifact)).toHaveLength(1);
    });

    gate.reject(boardRequestError("artifact_not_found", "Artifact read failed"));
    await expect.element(page.getByRole("alert")).toBeVisible();
    const retryBtn = page.getByRole("button", { name: /retry/i });
    await expect.element(retryBtn).toBeVisible();

    await userEvent.click(retryBtn);
    await expect.element(page.getByText("Recovered Artifact")).toBeVisible();
    expect(
      rpc.stub.calls.filter((c) => c.method === "workflowBoard.getArtifact").length,
    ).toBeGreaterThanOrEqual(2);
  });

  it("[F2-T2-04] maintains accessible non-clipped artifact and thread split panes at 1024x700 viewport", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true }, [
      threadWithHistory("split-1024", 1),
    ]);
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-1024",
        title: "Viewport 1024 Card",
        stage: "plan",
        threadId: "split-1024",
        artifacts: { "plan-brief.md": "Split 1024 content" },
      }),
    ]);
    await using _screen = await mountBoardAppAt("/", 1024, 700);
    await openBoardViaCommandPalette(rpc);

    await userEvent.click(page.getByText("Viewport 1024 Card"));
    const artifactEl = page.getByText("Split 1024 content");
    const chatEl = page.getByText("split-1024: question 0");
    await expect.element(artifactEl).toBeVisible();
    await expect.element(chatEl).toBeVisible();

    expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(1024);
    const artifactRect = artifactEl.element().getBoundingClientRect();
    const chatRect = chatEl.element().getBoundingClientRect();
    expect(artifactRect.width).toBeGreaterThan(0);
    expect(artifactRect.height).toBeGreaterThan(0);
    expect(chatRect.width).toBeGreaterThan(0);
    expect(chatRect.height).toBeGreaterThan(0);
    expect(artifactRect.left).toBeGreaterThanOrEqual(0);
    expect(artifactRect.right).toBeLessThanOrEqual(1024);
    expect(chatRect.left).toBeGreaterThanOrEqual(0);
    expect(chatRect.right).toBeLessThanOrEqual(1024);
    const nonOverlapping =
      artifactRect.right <= chatRect.left + 1 ||
      chatRect.right <= artifactRect.left + 1 ||
      artifactRect.bottom <= chatRect.top + 1 ||
      chatRect.bottom <= artifactRect.top + 1;
    expect(nonOverlapping).toBe(true);
  });

  it("[F3-T1-01] @dnd-kit pointer drag from Spec to Plan opens approval/transition confirmation UI", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({ cardId: "card-spec-1", title: "Spec Transition Card", revision: 3 }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    const cardEl = findCardDragHandle("card-spec-1");
    const planColEl = findColumnElement("Plan");
    {
      using _drag = await dispatchPointerDrag(cardEl, planColEl);
    }

    const dialog = page.getByRole("alertdialog");
    await expect.element(dialog).toBeVisible();
    expect(rpc.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(false);

    await userEvent.click(dialog.getByRole("button", { name: "Confirm spec approval" }));
    await vi.waitFor(() => {
      expect(
        rpc.stub.calls.some(
          (c) =>
            c.method === "workflowBoard.approveSpec" &&
            (c.input as { readonly cardId?: string; readonly expectedRevision?: number })
              ?.cardId === "card-spec-1" &&
            (c.input as { readonly expectedRevision?: number })?.expectedRevision === 3,
        ),
      ).toBe(true);
    });
  });

  it("[F3-T1-02] provides drag, button, and keyboard parity for cross-column stage transitions", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-parity",
        title: "Parity Card",
        revision: 2,
        nextAction: "approve_spec",
      }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    const cardEl = findCardDragHandle("card-parity");
    const planColEl = findColumnElement("Plan");

    // 1. Pointer drag opens confirmation dialog; cancel leaves RPC uncalled.
    {
      using _drag = await dispatchPointerDrag(cardEl, planColEl);
    }
    await expect.element(page.getByRole("alertdialog")).toBeVisible();
    await userEvent.click(page.getByRole("alertdialog").getByRole("button", { name: /cancel/i }));
    expect(rpc.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(false);

    // 2. Button click opens the same confirmation dialog; cancel leaves RPC uncalled.
    await page.getByRole("button", { name: "Move Parity Card to Plan" }).click();
    await expect.element(page.getByRole("alertdialog")).toBeVisible();
    await userEvent.click(page.getByRole("alertdialog").getByRole("button", { name: /cancel/i }));
    expect(rpc.stub.calls.some((c) => c.method === "workflowBoard.approveSpec")).toBe(false);

    // 3. Keyboard drag opens confirmation dialog and Confirm dispatches approveSpec.
    (cardSelectButton("Parity Card").element() as HTMLElement).focus();
    await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
    const dialog = page.getByRole("alertdialog");
    await expect.element(dialog).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Confirm spec approval" }));
    await vi.waitFor(() => {
      expect(
        rpc.stub.calls.filter(
          (c) =>
            c.method === "workflowBoard.approveSpec" &&
            (c.input as { readonly cardId?: string; readonly expectedRevision?: number })
              ?.cardId === "card-parity" &&
            (c.input as { readonly expectedRevision?: number })?.expectedRevision === 2,
        ),
      ).toHaveLength(1);
    });
  });

  it("[F3-T1-03] return-for-revision Plan -> Spec preserves history and displays Stale badge on Plan approval", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({
        cardId: "card-ret",
        title: "Return Revision Card",
        stage: "plan",
        revision: 5,
        specApprovalStatus: "valid",
        planApprovalStatus: "valid",
      }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    await page.getByRole("button", { name: "Return Return Revision Card for revision" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect.element(dialog).toBeVisible();
    await dialog.getByLabelText("Reason for revision").fill("Clarify AC-02");
    await dialog.getByRole("button", { name: "Confirm return for revision" }).click();

    await vi.waitFor(() => {
      expect(
        rpc.stub.calls.some((c) => {
          if (c.method !== "workflowBoard.returnForRevision") return false;
          const payload = c.input as {
            readonly cardId?: string;
            readonly expectedRevision?: number;
            readonly targetStage?: string;
            readonly reason?: string;
          };
          return (
            payload.cardId === "card-ret" &&
            payload.expectedRevision === 5 &&
            payload.targetStage === "spec" &&
            payload.reason === "Clarify AC-02"
          );
        }),
      ).toBe(true);
    });
    await expect.element(page.getByText("Plan approval is stale")).toBeVisible();
    await page.getByRole("tab", { name: /History/ }).click();
    await expect.element(page.getByText("Clarify AC-02")).toBeVisible();
  });

  it("[F3-T2-01] blocks disallowed skip-column drop Spec -> Ship with accessible guidance", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({ cardId: "card-skip", title: "No Skip Card", stage: "spec" }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    const cardEl = findCardDragHandle("card-skip");
    const shipColEl = findColumnElement("Ship");
    {
      using _drag = await dispatchPointerDrag(cardEl, shipColEl);
    }

    const mutations = rpc.stub.calls.filter(
      (c) =>
        c.method.startsWith("workflowBoard.approve") ||
        c.method === "workflowBoard.returnForRevision",
    );
    expect(mutations).toHaveLength(0);
    expect(findColumnElement("Spec").textContent).toContain("No Skip Card");
    await vi.waitFor(() => {
      const guidanceCount =
        page.getByRole("alert").elements().length + page.getByRole("status").elements().length;
      expect(guidanceCount).toBeGreaterThan(0);
    });
  });

  it("[F3-T2-05] aborting mid-drag via pointercancel or Escape returns card to origin column with zero RPC mutations", async () => {
    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using rpc = installBoardRpcFixtures([
      makeBoardCardFixture({ cardId: "card-abort", title: "Abort Drag Card", stage: "spec" }),
    ]);
    await using _screen = await mountBoardAppAt("/");
    await openBoardViaCommandPalette(rpc);

    const cardEl = findCardDragHandle("card-abort");
    const planColEl = findColumnElement("Plan");
    await dispatchPointerCancelDrag(cardEl, planColEl);

    (findCardDragHandle("card-abort") as HTMLElement).focus();
    await userEvent.keyboard(" ");
    await userEvent.keyboard("{ArrowRight}");
    await userEvent.keyboard("{Escape}");

    expect(page.getByRole("alertdialog").elements()).toHaveLength(0);
    const mutations = rpc.stub.calls.filter(
      (c) =>
        c.method.startsWith("workflowBoard.approve") ||
        c.method === "workflowBoard.returnForRevision",
    );
    expect(mutations).toHaveLength(0);
    expect(findColumnElement("Spec").textContent).toContain("Abort Drag Card");
  });
});

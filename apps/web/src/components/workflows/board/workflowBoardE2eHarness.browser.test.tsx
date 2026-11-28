import {
  DndContext,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { EnvironmentId } from "@legioncode/contracts";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vite-plus/test";

import "~/index.css";
import {
  createBoardRpcStub,
  dispatchPointerDrag,
  hasProjectWorkflowBoardCapability,
  makeBoardServerConfigFixture,
  renderWithBoardRouter,
  withDesktopViewport,
  withProjectWorkflowBoardCapability,
  type BoardServerConfig,
} from "./workflowBoardE2eHarness";

const envId = EnvironmentId.make("env-board-e2e");

const mockState = vi.hoisted(() => ({
  configs: new Map<EnvironmentId, BoardServerConfig>(),
  connectedIds: [] as Array<EnvironmentId>,
  environments: [] as Array<{ readonly environmentId: EnvironmentId; readonly label: string }>,
  search: {} as Record<string, unknown>,
  snapshot: { workflows: [], runs: [], questions: [] },
}));

function setupWorkflowPageMocks(options: {
  readonly workflowsEnabled: boolean;
  readonly projectWorkflowBoard: boolean;
}) {
  const config = makeBoardServerConfigFixture({
    environmentId: envId,
    workflowsEnabled: options.workflowsEnabled,
    projectWorkflowBoard: options.projectWorkflowBoard,
  });
  mockState.configs = new Map([[envId, config]]);
  mockState.connectedIds = [envId];
  mockState.environments = [{ environmentId: envId, label: "board-e2e-server" }];
  mockState.search = {};
  mockState.snapshot = { workflows: [], runs: [], questions: [] };
  return config;
}

vi.mock(import("@tanstack/react-router"), async (original) => {
  const actual = await original();
  return {
    ...actual,
    useSearch: (() => mockState.search) as never,
    useNavigate: (() => vi.fn()) as never,
  };
});

vi.mock(import("../../ui/sidebar"), async (original) => {
  const actual = await original();
  return {
    ...actual,
    SidebarInset: (({
      children,
      className,
    }: {
      children?: ReactNode | undefined;
      className?: string | undefined;
    }) => <div className={className}>{children}</div>) as typeof actual.SidebarInset,
  };
});

vi.mock(import("../../../state/entities"), async (original) => ({
  ...(await original()),
  useServerConfigs: () => mockState.configs as never,
}));

vi.mock(import("../../../state/environments"), async (original) => ({
  ...(await original()),
  usePrimaryEnvironmentId: () => envId,
  useConnectedEnvironmentIds: () => mockState.connectedIds,
  useEnvironments: () =>
    ({
      isReady: true,
      networkStatus: "online",
      environments: mockState.environments,
      presentationById: new Map(),
    }) as never,
  useEnvironment: ((id: EnvironmentId | null) =>
    mockState.environments.find((e) => e.environmentId === id) ?? null) as never,
}));

vi.mock(import("../../../state/query"), async (original) => ({
  ...(await original()),
  useEnvironmentQuery: (() => ({
    data: mockState.snapshot,
    error: null,
    isPending: false,
    refresh: vi.fn(),
  })) as never,
}));

import { WorkflowsPage } from "../WorkflowsPage";

function DraggableCard({ id }: { readonly id: string }) {
  const { attributes, listeners, setNodeRef } = useDraggable({ id });
  return (
    <div
      ref={setNodeRef}
      data-testid={`draggable-${id}`}
      style={{ width: 80, height: 40 }}
      {...attributes}
      {...listeners}
    >
      {id}
    </div>
  );
}

function DroppableCol({ id }: { readonly id: string }) {
  const { setNodeRef } = useDroppable({ id });
  return (
    <div ref={setNodeRef} data-testid={`droppable-${id}`} style={{ width: 120, height: 80 }}>
      {id}
    </div>
  );
}

function BoardDragFixture({ onDragEnd }: { readonly onDragEnd: (event: DragEndEvent) => void }) {
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));
  return (
    <DndContext sensors={sensors} onDragEnd={onDragEnd}>
      <div style={{ display: "flex", gap: 48, padding: 20 }}>
        <DroppableCol id="spec-col" />
        <DraggableCard id="card-1" />
        <DroppableCol id="plan-col" />
      </div>
    </DndContext>
  );
}

function ThrowOnRender(): ReactNode {
  throw new Error("render boom");
}

describe("workflowBoardE2eHarness", () => {
  it("probes capabilities and renders WorkflowsPage Labs gating cleanly", async () => {
    const legacy = setupWorkflowPageMocks({
      workflowsEnabled: false,
      projectWorkflowBoard: false,
    });
    expect(hasProjectWorkflowBoardCapability(legacy)).toBe(false);

    const enabledCap = withProjectWorkflowBoardCapability(legacy, true);
    expect(hasProjectWorkflowBoardCapability(enabledCap)).toBe(true);

    await using screen = await renderWithBoardRouter(<WorkflowsPage />, {
      route: "/workflows",
      width: 1280,
      height: 800,
    });
    await expect.element(screen.getByText("Workflows are off")).toBeVisible();
    await expect.element(screen.getByRole("link", { name: "Settings → Labs" })).toBeVisible();

    setupWorkflowPageMocks({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    await screen.rerender(<WorkflowsPage />);
    await expect.element(screen.getByText("Workflows are off")).not.toBeInTheDocument();
    await expect.element(screen.getByRole("heading", { name: "Workflows" })).toBeVisible();
  });

  it("restores viewport, resolves deferred RPC gates, and dispatches pointer drags", async () => {
    const initialWidth = window.innerWidth;
    {
      await using _vp = await withDesktopViewport(1100, 720);
      expect(window.innerWidth).toBe(1100);
    }
    expect(window.innerWidth).toBe(initialWidth);

    const stub = createBoardRpcStub();
    stub.on("workflowBoard.getCard", (input) => ({ ok: true, input }));
    const immediate = await stub.dispatch("workflowBoard.getCard", { cardId: "card-1" });
    expect(immediate).toEqual({ ok: true, input: { cardId: "card-1" } });

    const gate = stub.defer("workflowBoard.approveSpec");
    const pending = stub.dispatch("workflowBoard.approveSpec", { cardId: "card-1", revision: 2 });
    gate.resolve({ approved: true });
    expect(await pending).toEqual({ approved: true });
    expect(stub.calls).toHaveLength(2);

    // FIFO queued defer calls for repeated invocations of the same method.
    const gate1 = stub.defer("workflowBoard.updateSpec");
    const gate2 = stub.defer("workflowBoard.updateSpec");
    const p1 = stub.dispatch("workflowBoard.updateSpec", { revision: 1 });
    const p2 = stub.dispatch("workflowBoard.updateSpec", { revision: 2 });
    gate1.resolve({ step: 1 });
    gate2.resolve({ step: 2 });
    expect(await p1).toEqual({ step: 1 });
    expect(await p2).toEqual({ step: 2 });

    // clear() discards unconsumed deferred gates so subsequent dispatches do not stall.
    stub.defer("workflowBoard.getCard");
    stub.clear();
    expect(stub.calls).toHaveLength(0);
    expect(await stub.dispatch("workflowBoard.getCard", { cardId: "after-clear" })).toEqual({
      ok: true,
      input: { cardId: "after-clear" },
    });

    // Real @dnd-kit/core DndContext + PointerSensor drag test + single pointerup across disposal.
    const dragEnds: Array<{ readonly active: string; readonly over: string | null }> = [];
    let pointerUpCount = 0;
    const onPointerUp = () => {
      pointerUpCount += 1;
    };
    document.addEventListener("pointerup", onPointerUp);
    try {
      await using screen = await renderWithBoardRouter(
        <BoardDragFixture
          onDragEnd={(event) => {
            dragEnds.push({
              active: String(event.active.id),
              over: event.over ? String(event.over.id) : null,
            });
          }}
        />,
      );
      const sourceEl = screen.getByTestId("draggable-card-1").element();
      const targetEl = screen.getByTestId("droppable-plan-col").element();
      {
        using _drag = await dispatchPointerDrag(sourceEl, targetEl);
      }
      expect(dragEnds).toEqual([{ active: "card-1", over: "plan-col" }]);
      expect(pointerUpCount).toBe(1);
    } finally {
      document.removeEventListener("pointerup", onPointerUp);
    }

    // renderWithBoardRouter restores viewport if render throws on initial mount.
    using _consoleErr = vi.spyOn(console, "error").mockImplementation(() => {});
    const beforeThrowW = window.innerWidth;
    const beforeThrowH = window.innerHeight;
    await expect(
      renderWithBoardRouter(<ThrowOnRender />, { width: 911, height: 511 }),
    ).rejects.toThrow("render boom");
    expect(window.innerWidth).toBe(beforeThrowW);
    expect(window.innerHeight).toBe(beforeThrowH);
  });
});

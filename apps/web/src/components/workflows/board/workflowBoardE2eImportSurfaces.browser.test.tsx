import {
  EnvironmentId,
  PositiveInt,
  WorkflowBoardApprovalId,
  WorkflowBoardArtifactKey,
  WS_METHODS,
  type WorkflowBoardApplyCliImportResult,
  type WorkflowBoardApproveSpecResult,
  type WorkflowBoardPreviewCliImportResult,
} from "@legioncode/contracts";
import { DEFAULT_CLIENT_SETTINGS } from "@legioncode/contracts/settings";
import { createMemoryHistory } from "@tanstack/react-router";
import type * as Context from "effect/Context";
import * as Stream from "effect/Stream";
import { describe, expect, it, vi } from "vite-plus/test";
import { page, userEvent } from "vite-plus/test/browser";

import { AppRoot } from "~/AppRoot";
import { openCommandPalette } from "~/commandPaletteBus";
import { useCommandPaletteStore } from "~/commandPaletteStore";
import "~/components/ChatView";
import { __setClientSettingsForTests } from "~/hooks/useSettings";
import "~/index.css";
import { getRouter, type AppRouter } from "~/router";
import { appAtomRegistry } from "~/rpc/atomRegistry";
import {
  PROJECT_ID,
  answerRpc,
  installFakeEnvironment,
  threadWithHistory,
  type FakeDriver,
} from "../../../../test/fakeEnvironment";
import { act, render } from "../../../../test/render";
import {
  dispatchPointerDrag,
  hasProjectWorkflowBoardCapability,
  makeBoardServerConfigFixture,
  withDesktopViewport,
} from "./workflowBoardE2eHarness";
import {
  BOARD_AT,
  installWorkflowBoardFixtures,
  makeBoardCardDetail,
  type WorkflowBoardFixtureStore,
} from "./workflowBoardE2eFixtures";

const envId = EnvironmentId.make("env-board-e2e");

const boardEnvState = vi.hoisted(() => ({
  workflowsEnabled: false,
  projectWorkflowBoard: false,
  emitConfigSnapshot: null as (() => void) | null,
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
                  offerSnapshot();
                  boardEnvState.emitConfigSnapshot = () => {
                    void offerSnapshot();
                  };
                  yield* EffectMod.addFinalizer(() =>
                    EffectMod.sync(() => {
                      boardEnvState.emitConfigSnapshot = null;
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

function setupBoardAppEnvironment(options: {
  readonly workflowsEnabled: boolean;
  readonly projectWorkflowBoard: boolean;
}) {
  useCommandPaletteStore.setState(useCommandPaletteStore.getInitialState(), true);
  boardEnvState.workflowsEnabled = options.workflowsEnabled;
  boardEnvState.projectWorkflowBoard = options.projectWorkflowBoard;
  boardEnvState.emitConfigSnapshot?.();
  __setClientSettingsForTests({
    ...DEFAULT_CLIENT_SETTINGS,
    boringSidebarEnabled: true,
    onboardingCompletedAt: "2026-01-01T00:00:00.000Z",
  });
  return installFakeEnvironment([threadWithHistory("board-surface-thread", 1)]);
}

async function mountBoardAppAt(
  initialEntry: string,
  viewport: { readonly width: number; readonly height: number } = { width: 1280, height: 800 },
) {
  await using stack = new AsyncDisposableStack();
  appAtomRegistry.reset();
  stack.use(await withDesktopViewport(viewport.width, viewport.height));
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
    router,
    [Symbol.asyncDispose]: () => owned.disposeAsync(),
  };
}

function answerWorkflowRunsEmpty() {
  const sub = answerRpc(WS_METHODS.workflowsSubscribe, () =>
    Stream.make({ workflows: [], runs: [], questions: [] }),
  );
  const cnt = answerRpc(WS_METHODS.workflowsCounts, () =>
    Stream.make({ pendingQuestions: 0, running: 0 }),
  );
  return {
    [Symbol.dispose]() {
      cnt[Symbol.dispose]();
      sub[Symbol.dispose]();
    },
  };
}

const BOARD_STAGE_COLUMNS = ["Spec", "Plan", "Execute", "Ship"] as const;

const TRANSITION_METHODS: ReadonlySet<string> = new Set([
  WS_METHODS.workflowBoardApproveSpec,
  WS_METHODS.workflowBoardApprovePlan,
  WS_METHODS.workflowBoardApproveDelivery,
  WS_METHODS.workflowBoardReturnForRevision,
]);

/** Opens the shipped board through the "Open workflow board" palette action and waits for the 4 stage columns. */
async function openWorkflowBoardFromPalette(screen: {
  readonly router: AppRouter;
}): Promise<string> {
  const previousHref = screen.router.state.location.href;
  openCommandPalette();
  const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
  await expect.element(paletteInput).toBeVisible();
  await userEvent.fill(paletteInput, "Open workflows");
  await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
  await userEvent.fill(paletteInput, "workflow board");
  const option = page.getByRole("option", { name: /open workflow board/i });
  await expect.element(option).toBeVisible();
  await userEvent.click(option);

  await vi.waitFor(() => {
    expect(useCommandPaletteStore.getState().open).toBe(false);
    expect(screen.router.state.location.pathname).toMatch(/^\/projects\/[^/]+\/board$/);
    expect(screen.router.state.location.href).not.toBe(previousHref);
    expect(screen.router.state.status).toBe("idle");
  });
  await expect
    .element(page.getByRole("dialog", { name: "Command palette" }))
    .not.toBeInTheDocument();
  await expectFourStageColumns();
  return screen.router.state.location.href;
}

async function expectFourStageColumns() {
  for (const column of BOARD_STAGE_COLUMNS) {
    await expect.element(page.getByRole("region", { name: column, exact: true })).toBeVisible();
    await expect
      .element(page.getByRole("heading", { name: new RegExp(`^Open ${column} stage`) }))
      .toBeVisible();
  }
}

function installApproveSpecHandler(store: WorkflowBoardFixtureStore) {
  store.on(WS_METHODS.workflowBoardApproveSpec, (input, s): WorkflowBoardApproveSpecResult => {
    const card = s.get(String(input.cardId));
    expect(input.expectedRevision).toBe(card.revision);
    const revision = PositiveInt.make(card.revision + 1);
    const specApproval = {
      id: WorkflowBoardApprovalId.make(`approval-spec-${card.cardId}-${revision}`),
      gate: "spec" as const,
      status: "valid" as const,
      revision,
      artifactDigest: `sha256:spec-${card.cardId}-${revision}`,
      specDigest: card.spec.specDigest,
      wireframesDigest: null,
      approvedAt: BOARD_AT,
      approvedBy: "board-e2e-operator",
    };
    const next = s.advance(card.cardId, {
      stage: "plan",
      specApproval,
      specApprovalStatus: "valid",
      specApprovedRevision: revision,
      nextAction: "start_plan",
    });
    return { cardId: next.cardId, revision: next.revision, specApproval, card: next };
  });
}

const TSK_0002_COMMAND = "node --test packages/provider-mimo/qualification/cli.test.mjs";

function makeRealCliImportPreview(): WorkflowBoardPreviewCliImportResult {
  const task = (taskId: string, title: string, status: "completed" | "blocked" | "todo") => ({
    id: taskId,
    taskId,
    title,
    status,
    blockerSummary: status === "blocked" ? `Failed command: ${TSK_0002_COMMAND}` : null,
    verificationCommands:
      status === "blocked"
        ? [["node", "--test", "packages/provider-mimo/qualification/cli.test.mjs"] as const]
        : [],
    missingLogNotes: [],
  });
  const receipt = (kind: "spec_approval" | "plan_approval" | "execution", source: string) => ({
    id: `receipt-${kind}`,
    kind,
    status: "historical" as const,
    sourcePath: WorkflowBoardArtifactKey.make(`.legion-cli/specs/spec-t3-code/${source}`),
    recordedAt: null,
    recordedBy: null,
    summary: `Historical ${kind} receipt imported from ${source}`,
    requiresNativeRevalidation: true,
  });
  const policy = (key: string, category: string, blocksExecution: boolean) => ({
    key,
    category,
    summary: `${key} is not enforced by LegionCode`,
    configuredValue: "true",
    status: "unsupported" as const,
    blocksExecution,
    sourcePath: WorkflowBoardArtifactKey.make(".legion-cli/config.yaml"),
  });
  return {
    projectId: PROJECT_ID,
    sourcePath: WorkflowBoardArtifactKey.make(".legion-cli"),
    specId: "spec-t3-code",
    title: "T3 Code",
    stage: "execute",
    status: "blocked",
    existingCardId: null,
    alreadyImported: false,
    activeLocks: [],
    staleLocks: [],
    canApply: true,
    refusalReason: null,
    tasks: [
      task("TSK-0001", "Task 1", "completed"),
      task("TSK-0002", "Verify provider-mimo CLI", "blocked"),
      ...["TSK-0003", "TSK-0004", "TSK-0005", "TSK-0006", "TSK-0007", "TSK-0008"].map((id, i) =>
        task(id, `Task ${i + 3}`, "todo"),
      ),
    ],
    decisionIds: [],
    assumptionIds: [],
    acceptanceCriterionIds: [],
    historicalReceipts: [
      receipt("spec_approval", "spec-approval.yaml"),
      receipt("plan_approval", "plan-approval.yaml"),
      receipt("execution", "execution.yaml"),
    ],
    unsupportedPolicies: [
      policy("sandbox.requireHardened", "sandbox", true),
      policy("qa.passScore", "qa", false),
      policy("skills.trustKeys", "skills", false),
    ],
    missingLogNotes: [],
    warnings: [],
  };
}

describe("F23 multi-surface capability, entry, a11y & responsive layout (web)", () => {
  it("[F23-T1-01] preserves legacy /workflows run inspector and gates board UI when projectWorkflowBoard is false", async () => {
    const offConfig = makeBoardServerConfigFixture({
      environmentId: envId,
      workflowsEnabled: false,
      projectWorkflowBoard: false,
    });
    expect(hasProjectWorkflowBoardCapability(offConfig)).toBe(false);

    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: false,
        projectWorkflowBoard: false,
      });
      using _wf = answerWorkflowRunsEmpty();
      await using screen = await mountBoardAppAt("/workflows");
      await expect.element(screen.getByText("Workflows are off")).toBeVisible();
      await expect.element(screen.getByRole("link", { name: "Settings → Labs" })).toBeVisible();
    }

    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      using _wf = answerWorkflowRunsEmpty();
      await using screen = await mountBoardAppAt("/workflows");
      await expect.element(screen.getByRole("heading", { name: "Workflows" })).toBeVisible();
      await expect
        .element(screen.getByRole("button", { name: "Definitions" }).first())
        .toBeVisible();
    }

    let boardHref = "";
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: true,
      });
      using _board = installWorkflowBoardFixtures([]);
      await using screen = await mountBoardAppAt("/");
      boardHref = await openWorkflowBoardFromPalette(screen);
      await expect.element(page.getByRole("button", { name: "New card" }).first()).toBeEnabled();
    }

    using _legacyEnv = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: false,
    });
    using _wf = answerWorkflowRunsEmpty();
    using legacyBoard = installWorkflowBoardFixtures([]);
    {
      await using legacyBoardScreen = await mountBoardAppAt(boardHref);
      await expect
        .element(legacyBoardScreen.getByText("Workflow board is not supported"))
        .toBeVisible();
      await expect
        .element(
          legacyBoardScreen.getByText(
            "This server does not advertise project workflow board support yet.",
          ),
        )
        .toBeVisible();
      // Board operations are gated: no create/import/approve controls and no stage columns.
      expect(
        legacyBoardScreen
          .getByRole("button", { name: /new card|import cli project|approve/i })
          .elements(),
      ).toHaveLength(0);
      for (const column of BOARD_STAGE_COLUMNS) {
        expect(page.getByRole("region", { name: column, exact: true }).elements()).toHaveLength(0);
      }
      expect(legacyBoard.callsTo(WS_METHODS.workflowBoardSubscribe)).toHaveLength(0);
    }
    {
      await using legacyRunsScreen = await mountBoardAppAt("/workflows");
      await expect
        .element(legacyRunsScreen.getByRole("heading", { name: "Workflows" }))
        .toBeVisible();
    }
  });

  it("[F23-T1-02] verifies command palette baseline without board capability and opens 4-column board from sidebar & palette when enabled", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      using _wf = answerWorkflowRunsEmpty();
      {
        await using _home = await mountBoardAppAt("/");
        openCommandPalette();
        const paletteInput = page.getByPlaceholder("Search commands, projects, and threads...");
        await expect.element(paletteInput).toBeVisible();
        await userEvent.fill(paletteInput, "Open workflows");
        await expect.element(page.getByRole("option", { name: "Open workflows" })).toBeVisible();
        await userEvent.fill(paletteInput, "workflow board");
        await vi.waitFor(() => {
          expect(page.getByRole("option", { name: /Open workflows/ }).elements()).toHaveLength(0);
        });
        expect(page.getByRole("option", { name: /workflow board/i }).elements()).toHaveLength(0);
        await userEvent.keyboard("{Escape}");
      }

      {
        await using wfScreen = await mountBoardAppAt("/workflows");
        await expect.element(wfScreen.getByRole("heading", { name: "Workflows" })).toBeVisible();
      }
    }

    using _env = setupBoardAppEnvironment({
      workflowsEnabled: true,
      projectWorkflowBoard: true,
    });
    using _wf = answerWorkflowRunsEmpty();
    using _board = installWorkflowBoardFixtures([]);

    {
      await using screen = await mountBoardAppAt("/");
      await openWorkflowBoardFromPalette(screen);
    }

    {
      await using sidebarHome = await mountBoardAppAt("/");
      const sidebarEntry = sidebarHome.getByRole("button", { name: /^workflow board for /i });
      await expect.element(sidebarEntry).toBeInTheDocument();
      // The project row action reveals on hover, like the other sidebar row actions.
      await userEvent.hover(sidebarEntry);
      await expect.element(sidebarEntry).toBeVisible();
      await userEvent.click(sidebarEntry);
      await vi.waitFor(() => {
        expect(sidebarHome.router.state.location.pathname).toMatch(/^\/projects\/[^/]+\/board$/);
      });
      await expectFourStageColumns();
    }

    {
      await using runsScreen = await mountBoardAppAt("/workflows");
      await expect.element(runsScreen.getByRole("heading", { name: "Workflows" })).toBeVisible();
      await expect
        .element(runsScreen.getByRole("button", { name: "Definitions" }).first())
        .toBeVisible();
    }
  });

  it("[F23-T1-03] exposes ARIA landmarks and submits the same transition from keyboard and pointer", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const card = makeBoardCardDetail({ cardId: "parity", title: "Keyboard Parity Card" });
    const expected = { cardId: card.cardId, expectedRevision: card.revision };

    for (const input of ["keyboard", "pointer"] as const) {
      using store = installWorkflowBoardFixtures([card]);
      installApproveSpecHandler(store);
      await using screen = await mountBoardAppAt("/");
      await openWorkflowBoardFromPalette(screen);
      if (input === "keyboard") {
        const select = screen
          .getByRole("region", { name: "Spec", exact: true })
          .getByRole("button", { name: /^Keyboard Parity Card/ });
        await expect.element(select).toHaveAttribute("aria-keyshortcuts", "Alt+ArrowRight");
        await act(() => select.element().focus());
        await expect.element(select).toHaveFocus();
        await userEvent.keyboard("{Alt>}{ArrowRight}{/Alt}");
      } else {
        const drag = screen.getByRole("button", {
          name: "Drag Keyboard Parity Card to another stage",
        });
        await expect.element(drag).toBeEnabled();
        const plan = screen.getByRole("region", { name: "Plan", exact: true });
        using _drag = await dispatchPointerDrag(drag.element(), plan.element());
      }
      const dialog = page.getByRole("alertdialog", { name: "Approve spec and move to Plan" });
      await expect
        .element(dialog)
        .toBeVisible()
        .catch((cause: unknown) => {
          throw new Error(`The ${input} action did not open the Spec approval dialog`, { cause });
        });
      expect(store.callsTo(WS_METHODS.workflowBoardApproveSpec)).toHaveLength(0);
      await dialog.getByRole("button", { name: "Confirm spec approval" }).click();
      await vi.waitFor(() => {
        const calls = store.callsTo(WS_METHODS.workflowBoardApproveSpec);
        expect(calls).toHaveLength(1);
        expect(calls[0]).toMatchObject(expected);
      });
    }
  });

  it("[F23-T2-01] isolates mixed-capability and disconnected environments with zero card leakage", async () => {
    expect(hasProjectWorkflowBoardCapability(null)).toBe(false);
    expect(hasProjectWorkflowBoardCapability(undefined)).toBe(false);
    const title = "Capable Environment Secret Card";
    const card = makeBoardCardDetail({ cardId: "capable-only", title });
    let boardHref: string;

    {
      using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
      using _wf = answerWorkflowRunsEmpty();
      {
        await using away = await mountBoardAppAt("/workflows?env=env-disconnected-remote");
        await expect
          .element(away.getByText("env-disconnected-remote is not connected"))
          .toBeVisible();
        await expect.element(away.getByText(title)).not.toBeInTheDocument();
      }
      using _board = installWorkflowBoardFixtures([card]);
      await using screen = await mountBoardAppAt("/");
      boardHref = await openWorkflowBoardFromPalette(screen);
      await expect.element(screen.getByText(title)).toBeVisible();
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: false });
    using board = installWorkflowBoardFixtures([card]);
    await using legacy = await mountBoardAppAt(boardHref);
    await expect.element(legacy.getByText("Workflow board is not supported")).toBeVisible();
    await expect.element(legacy.getByText(title)).not.toBeInTheDocument();
    expect(board.callsTo(WS_METHODS.workflowBoardSubscribe)).toHaveLength(0);
  });

  it("[F23-T2-02] restores viewport and switches narrow stage tabs without overflow or a mutation after drag cancellation", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    const card = makeBoardCardDetail({ cardId: "narrow", title: "Narrow Viewport Card" });
    using store = installWorkflowBoardFixtures([card]);
    const startingWidth = window.innerWidth;

    {
      await using screen = await mountBoardAppAt("/", { width: 390, height: 844 });
      await openWorkflowBoardFromPalette(screen);
      for (const name of ["Plan", "Execute", "Ship", "Spec"]) {
        const tab = screen.getByRole("tab", { name: new RegExp(`^${name}`) }).first();
        await tab.click();
        await expect.element(tab).toHaveAttribute("aria-selected", "true");
        expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(390);
      }
      await screen.getByRole("tab", { name: "Overview" }).click();
      const handle = screen.getByRole("button", {
        name: "Drag Narrow Viewport Card to another stage",
      });
      (handle.element() as HTMLElement).focus();
      await userEvent.keyboard(" {ArrowRight}{Escape}");
      await expect
        .element(screen.getByRole("region", { name: "Spec" }).getByText(card.title))
        .toBeVisible();
      expect(store.calls.filter((call) => TRANSITION_METHODS.has(call.method))).toHaveLength(0);
    }
    expect(window.innerWidth).toBe(startingWidth);
  });
});

describe("F24 browser CLI import preview, apply, blockers & capability gating (web)", () => {
  it("[F24-T1-06] previews spec-t3-code, TSK-0002 blocker & unsupported policies, and applies import into board", async () => {
    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using store = installWorkflowBoardFixtures([]);
    const preview = makeRealCliImportPreview();
    store.on(WS_METHODS.workflowBoardPreviewCliImport, () => preview);
    store.on(
      WS_METHODS.workflowBoardApplyCliImport,
      (input, fixture): WorkflowBoardApplyCliImportResult => {
        expect(input.projectId).toBe(PROJECT_ID);
        const card = fixture.put(
          makeBoardCardDetail({
            cardId: "card-spec-t3-code",
            title: preview.title,
            stage: preview.stage,
            status: preview.status,
            blockerSummary: preview.tasks[1]?.blockerSummary ?? null,
            nextAction: "retry_task",
          }),
        );
        return {
          card,
          cardId: card.cardId,
          projectId: card.projectId,
          revision: card.revision,
          created: true,
          sourceSpecId: preview.specId,
          tasks: [],
          historicalReceipts: preview.historicalReceipts,
          unsupportedPolicies: preview.unsupportedPolicies,
          missingLogNotes: preview.missingLogNotes,
        };
      },
    );

    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoardFromPalette(screen);
    await screen.getByRole("button", { name: "Import CLI project" }).click();
    const dialog = page.getByRole("dialog");
    for (const text of [
      "spec-t3-code",
      "TSK-0002",
      ...preview.unsupportedPolicies.map((p) => p.key),
    ]) {
      await expect.element(dialog.getByText(text, { exact: true })).toBeVisible();
    }
    await expect.element(dialog.getByText(`Failed command: ${TSK_0002_COMMAND}`)).toBeVisible();
    await expect
      .element(dialog.getByRole("heading", { name: /Historical receipts/ }))
      .toBeVisible();
    await dialog.getByRole("button", { name: "Apply import" }).click();
    await expect.element(dialog).not.toBeInTheDocument();
    expect(store.callsTo(WS_METHODS.workflowBoardApplyCliImport)).toHaveLength(1);
    await expect.element(screen.getByRole("tab", { name: /^Execute/ }).first()).toBeVisible();
    await expect.element(screen.getByRole("button", { name: /^T3 Code/ }).first()).toBeVisible();
  });

  it("[F24-T2-07] hides CLI import on legacy server and refuses active-lock takeover without ghost cards", async () => {
    {
      using _env = setupBoardAppEnvironment({
        workflowsEnabled: true,
        projectWorkflowBoard: false,
      });
      using _wf = answerWorkflowRunsEmpty();
      await using legacy = await mountBoardAppAt("/workflows");
      await expect.element(legacy.getByRole("heading", { name: "Workflows" })).toBeVisible();
      await expect
        .element(legacy.getByRole("button", { name: "Import CLI project" }))
        .not.toBeInTheDocument();
    }

    using _env = setupBoardAppEnvironment({ workflowsEnabled: true, projectWorkflowBoard: true });
    using store = installWorkflowBoardFixtures([]);
    const refusal = "Stop the active CLI run before importing into LegionCode.";
    store.on(
      WS_METHODS.workflowBoardPreviewCliImport,
      () =>
        ({
          ...makeRealCliImportPreview(),
          canApply: false,
          refusalReason: refusal,
          activeLocks: [
            {
              kind: "engine_lock",
              relativePath: WorkflowBoardArtifactKey.make(".legion-cli/index/engine.lock"),
              status: "active",
              pid: PositiveInt.make(123),
              runId: "run-active",
              startedAt: BOARD_AT,
              message: refusal,
            },
          ],
        }) satisfies WorkflowBoardPreviewCliImportResult,
    );

    await using screen = await mountBoardAppAt("/");
    await openWorkflowBoardFromPalette(screen);
    await screen.getByRole("button", { name: "Import CLI project" }).click();
    const dialog = page.getByRole("dialog");
    await expect
      .element(dialog.getByText("Active CLI run detected: takeover refused"))
      .toBeVisible();
    await expect.element(dialog.getByText(refusal)).toBeVisible();
    await expect.element(dialog.getByRole("button", { name: "Apply import" })).toBeDisabled();
    expect(store.callsTo(WS_METHODS.workflowBoardApplyCliImport)).toHaveLength(0);
    expect(store.cards.size).toBe(0);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect.element(screen.getByText("T3 Code")).not.toBeInTheDocument();
  });
});

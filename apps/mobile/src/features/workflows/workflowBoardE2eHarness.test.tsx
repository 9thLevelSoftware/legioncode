import { EnvironmentId } from "@legioncode/contracts";
import { AsyncResult } from "effect/reactivity";
import { useEffect, type ReactNode } from "react";
import { describe, expect, it, vi } from "vite-plus/test";

import {
  createMobileCommandRecorder,
  createMobileNavigationStub,
  hasMobileProjectWorkflowBoardCapability,
  makeMobileBoardEnvironment,
  mountMobileTestTree,
  type MobileBoardEnvironmentFixture,
} from "./workflowBoardE2eHarness";

const navHolder = vi.hoisted(() => ({
  navigation: null as ReturnType<typeof createMobileNavigationStub>["navigation"] | null,
}));

const envHolder = vi.hoisted(() => ({
  environments: [] as ReadonlyArray<MobileBoardEnvironmentFixture>,
  liveEntries: [] as ReadonlyArray<{
    readonly environmentId: EnvironmentId;
    readonly result: AsyncResult.AsyncResult<unknown, unknown>;
  }>,
}));

vi.mock("react-native", () => ({
  View: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
  Pressable: ({
    children,
    onPress,
    accessibilityLabel,
  }: {
    children?: ReactNode;
    onPress?: () => void;
    accessibilityLabel?: string;
  }) => (
    <button type="button" aria-label={accessibilityLabel} onClick={onPress}>
      {children}
    </button>
  ),
}));

vi.mock("react-native-safe-area-context", () => ({
  useSafeAreaInsets: () => ({ top: 0, right: 0, bottom: 0, left: 0 }),
}));

vi.mock("@react-navigation/native", () => ({
  StackActions: {
    push: (name: string, params?: unknown) => ({ type: "PUSH", payload: { name, params } }),
  },
  useNavigation: () => navHolder.navigation ?? createMobileNavigationStub().navigation,
}));

vi.mock("../../state/server", () => ({
  serverEnvironment: {
    workflowsLiveAcross: (key: string) => key,
  },
}));

vi.mock("../../state/query", () => ({
  formatEnvironmentQueryError: (cause: unknown) => String(cause),
}));

vi.mock("@effect/atom-react", () => ({
  useAtomValue: () => envHolder.liveEntries,
}));

vi.mock("../../components/RelativeTime", () => ({
  RelativeTime: ({ timestamp }: { readonly timestamp: string }) => <span>{timestamp}</span>,
}));
vi.mock("../../components/ui/AppText", () => ({
  AppText: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
}));
vi.mock("../../components/ui/EmptyState", () => ({
  EmptyState: ({ title, detail }: { readonly title: string; readonly detail: string }) => (
    <div data-testid="empty-state">
      <h2>{title}</h2>
      <p>{detail}</p>
    </div>
  ),
}));
vi.mock("../../components/ui/ScreenScrollView", () => ({
  ScreenScrollView: ({ children }: { children?: ReactNode }) => <div>{children}</div>,
}));
vi.mock("../../components/ui/SegmentedControl", () => ({
  SegmentedControl: ({
    options,
    onSelect,
  }: {
    readonly options: ReadonlyArray<{ readonly value: string; readonly label: string }>;
    readonly onSelect: (value: string) => void;
  }) => (
    <div>
      {options.map((opt) => (
        <button key={opt.value} type="button" onClick={() => onSelect(opt.value)}>
          {opt.label}
        </button>
      ))}
    </div>
  ),
}));
vi.mock("../../state/environments", () => ({
  useEnvironments: () => ({
    isReady: true,
    networkStatus: "online",
    environments: envHolder.environments,
    presentationById: new Map(),
  }),
}));

import { StackActions } from "@react-navigation/native";
import { WorkflowsScreen } from "./WorkflowsScreen";

describe("workflowBoardE2eHarness (mobile)", () => {
  it("evaluates capability gating across legacy, disconnected, and board-capable servers", () => {
    const legacy = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("legacy-mac"),
      projectWorkflowBoard: false,
    });
    const offline = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("offline-mac"),
      phase: "offline",
      projectWorkflowBoard: true,
    });
    const emptyConfig = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("pending-mac"),
      omitServerConfig: true,
    });
    const capable = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("board-mac"),
      projectWorkflowBoard: true,
    });

    expect(hasMobileProjectWorkflowBoardCapability([legacy, offline, emptyConfig])).toBe(false);
    expect(hasMobileProjectWorkflowBoardCapability([legacy, capable])).toBe(true);
  });

  it("mounts WorkflowsScreen and exercises navigation and command recorders", async () => {
    const env = makeMobileBoardEnvironment({
      environmentId: EnvironmentId.make("mobile-board-env"),
      projectWorkflowBoard: true,
    });
    const navStub = createMobileNavigationStub({ cardId: "card-1" });
    navHolder.navigation = navStub.navigation;
    envHolder.environments = [env];
    envHolder.liveEntries = [
      {
        environmentId: env.environmentId,
        result: AsyncResult.success({ workflows: [], runs: [], questions: [] }),
      },
    ];

    using mounted = mountMobileTestTree(<WorkflowsScreen />);
    expect(mounted.container.textContent).toContain("No workflow runs yet");

    const focusSpy = vi.fn();
    const unsub = navStub.navigation.addListener("focus", focusSpy);
    navStub.emit("focus");
    expect(focusSpy).toHaveBeenCalledTimes(1);
    unsub();
    navStub.navigation.dispatch(StackActions.push("WorkflowBoardCard", { cardId: "card-1" }));
    navStub.navigation.navigate("Thread", { threadId: "t-1" });
    navStub.navigation.goBack();
    expect(navStub.dispatched).toHaveLength(1);
    expect(navStub.navigated).toEqual([{ name: "Thread", params: { threadId: "t-1" } }]);
    expect(navStub.getGoBackCount()).toBe(1);

    const recorder = createMobileCommandRecorder();
    const approve = recorder.wrap("workflowBoard.approveDelivery");
    const gate = recorder.deferNext("workflowBoard.approveDelivery");
    const pending = approve({ cardId: "card-1", revision: 3 });
    gate.resolve({ _tag: "Success", value: { shipped: true } });
    expect(await pending).toEqual({ _tag: "Success", value: { shipped: true } });
    expect(recorder.calls).toEqual([
      { label: "workflowBoard.approveDelivery", input: { cardId: "card-1", revision: 3 } },
    ]);
    recorder.reset();
    expect(recorder.calls).toHaveLength(0);
  });

  it("restores IS_REACT_ACT_ENVIRONMENT when render or unmount throws", () => {
    const actEnv = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    const originalAct = actEnv.IS_REACT_ACT_ENVIRONMENT;
    using _consoleErr = vi.spyOn(console, "error").mockImplementation(() => {});

    const ThrowRender = (): ReactNode => {
      throw new Error("mobile render boom");
    };
    const ThrowCleanup = () => {
      useEffect(
        () => () => {
          throw new Error("mobile unmount boom");
        },
        [],
      );
      return <span>mounted</span>;
    };

    try {
      actEnv.IS_REACT_ACT_ENVIRONMENT = false;
      expect(() => mountMobileTestTree(<ThrowRender />)).toThrow("mobile render boom");
      expect(actEnv.IS_REACT_ACT_ENVIRONMENT).toBe(false);

      expect(() => {
        using mounted = mountMobileTestTree(<ThrowCleanup />);
        expect(mounted.container.textContent).toBe("mounted");
      }).toThrow("mobile unmount boom");
      expect(actEnv.IS_REACT_ACT_ENVIRONMENT).toBe(false);
    } finally {
      actEnv.IS_REACT_ACT_ENVIRONMENT = originalAct;
    }
  });
});

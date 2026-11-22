import {
  AuthTerminalOperateScope,
  type EnvironmentId,
  type ResolvedKeybindingsConfig,
  type ScopedThreadRef,
  type ThreadId,
} from "@legioncode/contracts";
import { scopedThreadKey, scopeProjectRef } from "@legioncode/client-runtime/environment";
import { projectScriptCwd, projectScriptRuntimeEnv } from "@legioncode/shared/projectScripts";
import { resolveTerminalSessionLabel } from "@legioncode/shared/terminalLabelsCopy";
import { useMessages } from "gt-react";
import { memo, useEffect, useState } from "react";
import { useComposerDraftStore } from "../composerDraftStore";
import { useStableCallback } from "../hooks/useStableCallback";
import type { TerminalContextSelection } from "../lib/terminalContext";
import { focusActiveDrawerTerminal, focusTerminal } from "../lib/terminalFocus";
import { cn } from "~/lib/utils";
import {
  selectThreadRightPanelState,
  useRightPanelStore,
  type RightPanelSurface,
} from "../rightPanelStore";
import { useProject, useQuietThreadShell } from "../state/entities";
import { useEnvironmentScope } from "../state/session";
import { useKnownTerminalSessions, useTerminalDrawerSessions } from "../state/terminalSessions";
import { selectThreadTerminalUiState, useTerminalUiStateStore } from "../terminalUiStateStore";
import { useThreadTerminalActions } from "./terminal/useThreadTerminalActions";
import ThreadTerminalDrawer from "./ThreadTerminalDrawer";
import { resolveTerminalWorktreePath } from "./ChatView.logic";

/** Where a drawer or panel opens new terminals when a script run overrides the thread's own. */
export type PersistentTerminalLaunchContext = {
  readonly cwd: string;
  readonly worktreePath: string | null;
};

interface TerminalLaunchLocationWithEnv {
  readonly cwd: string;
  readonly worktreePath: string | null;
  readonly runtimeEnv: Record<string, string>;
}

interface KnownTerminalSessionSummary {
  readonly target: { readonly terminalId: string };
  readonly state: {
    readonly summary: { readonly cwd: string; readonly worktreePath: string | null } | null;
  };
}

function launchLocationsEqual(
  left: TerminalLaunchLocationWithEnv,
  right: TerminalLaunchLocationWithEnv,
): boolean {
  if (left.cwd !== right.cwd || left.worktreePath !== right.worktreePath) {
    return false;
  }
  const leftKeys = Object.keys(left.runtimeEnv);
  const rightKeys = Object.keys(right.runtimeEnv);
  if (leftKeys.length !== rightKeys.length) {
    return false;
  }
  return leftKeys.every((key) => left.runtimeEnv[key] === right.runtimeEnv[key]);
}

function useResolvedTerminalLaunchLocations(input: {
  readonly threadKey: string;
  readonly openTerminalIds: readonly string[];
  readonly knownSessions: readonly KnownTerminalSessionSummary[] | undefined;
  readonly project: { readonly workspaceRoot: string } | null;
  readonly defaultLocation: TerminalLaunchLocationWithEnv | null;
}): Map<string, TerminalLaunchLocationWithEnv> {
  const [cachedState, setCachedState] = useState<{
    readonly threadKey: string;
    readonly locations: ReadonlyMap<string, TerminalLaunchLocationWithEnv>;
  }>(() => ({
    threadKey: input.threadKey,
    locations: new Map(),
  }));

  const previousLocations =
    cachedState.threadKey === input.threadKey ? cachedState.locations : null;
  const resolvedLocations = new Map<string, TerminalLaunchLocationWithEnv>();
  if (input.project) {
    for (const terminalId of input.openTerminalIds) {
      const summary =
        input.knownSessions?.find((session) => session.target.terminalId === terminalId)?.state
          .summary ?? null;
      const previousLocation = previousLocations?.get(terminalId);
      if (summary) {
        const nextLocation: TerminalLaunchLocationWithEnv = {
          cwd: summary.cwd,
          worktreePath: summary.worktreePath,
          runtimeEnv: projectScriptRuntimeEnv({
            project: { cwd: input.project.workspaceRoot },
            worktreePath: summary.worktreePath,
          }),
        };
        resolvedLocations.set(
          terminalId,
          previousLocation && launchLocationsEqual(previousLocation, nextLocation)
            ? previousLocation
            : nextLocation,
        );
        continue;
      }
      const nextLocation = previousLocation ?? input.defaultLocation;
      if (nextLocation) {
        resolvedLocations.set(terminalId, nextLocation);
      }
    }
  }

  let changed = previousLocations === null || previousLocations.size !== resolvedLocations.size;
  if (!changed && previousLocations !== null) {
    for (const [terminalId, nextLocation] of resolvedLocations) {
      const previousLocation = previousLocations.get(terminalId);
      if (!previousLocation || !launchLocationsEqual(previousLocation, nextLocation)) {
        changed = true;
        break;
      }
    }
  }
  if (changed) {
    setCachedState({
      threadKey: input.threadKey,
      locations: resolvedLocations,
    });
  }

  return resolvedLocations;
}

interface PersistentThreadTerminalDrawerProps {
  threadRef: { environmentId: EnvironmentId; threadId: ThreadId };
  threadId: ThreadId;
  active: boolean;
  composerHeight: number;
  launchContext: PersistentTerminalLaunchContext | null;
  splitShortcutLabel: string | undefined;
  splitVerticalShortcutLabel: string | undefined;
  newShortcutLabel: string | undefined;
  closeShortcutLabel: string | undefined;
  keybindings: ResolvedKeybindingsConfig;
  onAddTerminalContext: (selection: TerminalContextSelection) => void;
}

export const PersistentThreadTerminalDrawer = memo(function PersistentThreadTerminalDrawer({
  threadRef,
  threadId,
  active,
  composerHeight,
  launchContext,
  splitShortcutLabel,
  splitVerticalShortcutLabel,
  newShortcutLabel,
  closeShortcutLabel,
  keybindings,
  onAddTerminalContext,
}: PersistentThreadTerminalDrawerProps) {
  const messages = useMessages();
  const canOperateTerminal = useEnvironmentScope(threadRef.environmentId, AuthTerminalOperateScope);
  const terminalActions = useThreadTerminalActions(threadRef);
  const serverThread = useQuietThreadShell(threadRef);
  const draftThread = useComposerDraftStore((store) => store.getDraftThreadByRef(threadRef));
  const projectRef = serverThread
    ? scopeProjectRef(serverThread.environmentId, serverThread.projectId)
    : draftThread
      ? scopeProjectRef(draftThread.environmentId, draftThread.projectId)
      : null;
  const project = useProject(projectRef);
  const terminalUiState = useTerminalUiStateStore((state) =>
    selectThreadTerminalUiState(state.terminalUiStateByThreadKey, threadRef),
  );
  const visible = active && terminalUiState.terminalOpen;
  const metadata = useTerminalDrawerSessions(threadRef.environmentId, threadId);
  const knownTerminalSessions = metadata?.sessions;
  const snapshotId = metadata?.snapshotId;
  const panelSurfaces = useRightPanelStore(
    (state) => selectThreadRightPanelState(state.byThreadKey, threadRef).surfaces,
  );
  const panelTerminalIds = new Set(
    panelSurfaces.flatMap((surface) => (surface.kind === "terminal" ? surface.terminalIds : [])),
  );
  const drawerTerminalSessions =
    knownTerminalSessions?.filter((session) => !panelTerminalIds.has(session.target.terminalId)) ??
    [];
  const terminalLabelsById = new Map<string, string>();
  for (const session of drawerTerminalSessions) {
    terminalLabelsById.set(
      session.target.terminalId,
      resolveTerminalSessionLabel(session.target.terminalId, session.state.summary, messages),
    );
  }
  const serverOrderedTerminalIds = drawerTerminalSessions.map(
    (session) => session.target.terminalId,
  );
  const storeSetTerminalHeight = useTerminalUiStateStore((state) => state.setTerminalHeight);
  const storeSplitTerminal = useTerminalUiStateStore((state) => state.splitTerminal);
  const storeSplitTerminalVertical = useTerminalUiStateStore(
    (state) => state.splitTerminalVertical,
  );
  const storeNewTerminal = useTerminalUiStateStore((state) => state.newTerminal);
  const storeSetActiveTerminal = useTerminalUiStateStore((state) => state.setActiveTerminal);
  const storeCloseTerminal = useTerminalUiStateStore((state) => state.closeTerminal);
  const reconcileTerminalIds = useTerminalUiStateStore((state) => state.reconcileTerminalIds);

  // oxlint-disable-next-line legioncode/no-unreasoned-state-sync -- feeds the server's terminal list into the terminal UI store, which owns how local ids follow it
  useEffect(() => {
    if (snapshotId === undefined) return;
    reconcileTerminalIds(threadRef, serverOrderedTerminalIds, snapshotId);
  }, [reconcileTerminalIds, serverOrderedTerminalIds, snapshotId, threadRef]);
  const worktreePath = serverThread?.worktreePath ?? draftThread?.worktreePath ?? null;
  const effectiveWorktreePath = launchContext !== null ? launchContext.worktreePath : worktreePath;
  const cwd =
    launchContext?.cwd ??
    (project
      ? projectScriptCwd({
          project: { cwd: project.workspaceRoot },
          worktreePath: effectiveWorktreePath,
        })
      : null);
  const runtimeEnv = project
    ? projectScriptRuntimeEnv({
        project: { cwd: project.workspaceRoot },
        worktreePath: effectiveWorktreePath,
      })
    : {};
  const shouldRenderDrawer = Boolean(project && (terminalUiState.terminalOpen || active) && cwd);
  const drawerTerminalIds = canOperateTerminal
    ? terminalUiState.terminalIds
    : serverOrderedTerminalIds;
  const terminalLaunchLocationsById = useResolvedTerminalLaunchLocations({
    threadKey: scopedThreadKey(threadRef),
    openTerminalIds: shouldRenderDrawer ? drawerTerminalIds : [],
    knownSessions: knownTerminalSessions,
    project,
    defaultLocation:
      project && cwd ? { cwd, worktreePath: effectiveWorktreePath, runtimeEnv } : null,
  });

  const setTerminalHeight = (height: number) => {
    storeSetTerminalHeight(threadRef, height);
  };

  const launchDrawerTerminal = (place: (terminalId: string) => void) => {
    if (!cwd) return;
    const launched = terminalActions.launchTerminal({
      place,
      location: { cwd, worktreePath: effectiveWorktreePath },
    });
    if (!launched) return;
    if (visible) focusTerminal(threadRef, launched.terminalId);
    void launched.result;
  };
  const splitTerminal = useStableCallback(() =>
    launchDrawerTerminal((terminalId) => storeSplitTerminal(threadRef, terminalId)),
  );
  const splitTerminalVertical = useStableCallback(() =>
    launchDrawerTerminal((terminalId) => storeSplitTerminalVertical(threadRef, terminalId)),
  );
  const createNewTerminal = useStableCallback(() =>
    launchDrawerTerminal((terminalId) => storeNewTerminal(threadRef, terminalId)),
  );

  const activateTerminal = (terminalId: string) => {
    storeSetActiveTerminal(threadRef, terminalId);
    if (visible) focusTerminal(threadRef, terminalId);
  };

  const closeTerminal = useStableCallback((terminalId: string) => {
    if (!terminalActions.canWrite()) return;
    terminalActions.closeTerminalSession(terminalId);
    storeCloseTerminal(threadRef, terminalId);
    if (visible) focusActiveDrawerTerminal(threadRef);
  });

  const handleAddTerminalContext = (selection: TerminalContextSelection) => {
    if (!visible) {
      return;
    }
    onAddTerminalContext(selection);
  };

  if (!project || (!terminalUiState.terminalOpen && !active) || !cwd) {
    return null;
  }

  return (
    <div
      className={cn(
        "grid shrink-0 overflow-clip",
        active ? (visible ? "grid-rows-[1fr]" : "grid-rows-[0fr]") : "hidden",
        active &&
          "[[data-panel-animations=true]_&]:transition-[grid-template-rows] [[data-panel-animations=true]_&]:duration-(--panel-animation-duration) [[data-panel-animations=true]_&]:ease-out",
        active && visible && "[[data-panel-animations=true]_&]:starting:grid-rows-[0fr]!",
      )}
    >
      <div className="min-h-0 overflow-clip">
        <ThreadTerminalDrawer
          threadRef={threadRef}
          threadId={threadId}
          cwd={cwd}
          worktreePath={effectiveWorktreePath}
          runtimeEnv={runtimeEnv}
          visible={visible}
          height={terminalUiState.terminalHeight}
          composerHeight={composerHeight}
          // Known-session order is MRU and changes on focus; persisted store order keeps sidebar labels stable.
          terminalIds={canOperateTerminal ? terminalUiState.terminalIds : serverOrderedTerminalIds}
          activeTerminalId={terminalUiState.activeTerminalId}
          terminalGroups={terminalUiState.terminalGroups}
          activeTerminalGroupId={terminalUiState.activeTerminalGroupId}
          onSplitTerminal={splitTerminal}
          onSplitTerminalVertical={splitTerminalVertical}
          onNewTerminal={createNewTerminal}
          splitShortcutLabel={visible ? splitShortcutLabel : undefined}
          splitVerticalShortcutLabel={visible ? splitVerticalShortcutLabel : undefined}
          newShortcutLabel={visible ? newShortcutLabel : undefined}
          closeShortcutLabel={visible ? closeShortcutLabel : undefined}
          keybindings={keybindings}
          onActiveTerminalChange={activateTerminal}
          onCloseTerminal={closeTerminal}
          onHeightChange={setTerminalHeight}
          onAddTerminalContext={handleAddTerminalContext}
          terminalLabelsById={terminalLabelsById}
          terminalLaunchLocationsById={terminalLaunchLocationsById}
        />
      </div>
    </div>
  );
});

export interface PersistentThreadTerminalPanelProps {
  visible: boolean;
  threadRef: ScopedThreadRef;
  surface: Extract<RightPanelSurface, { kind: "terminal" }>;
  launchContext: PersistentTerminalLaunchContext | null;
  keybindings: ResolvedKeybindingsConfig;
  onAddTerminalContext: (selection: TerminalContextSelection) => void;
  onSplitTerminal: () => void;
  onSplitTerminalVertical: () => void;
  onNewTerminal: () => void;
  onActiveTerminalChange: (terminalId: string) => void;
  onCloseTerminal: (terminalId: string, threadRef?: ScopedThreadRef) => void;
  splitShortcutLabel?: string | undefined;
  splitVerticalShortcutLabel?: string | undefined;
  newShortcutLabel?: string | undefined;
  closeShortcutLabel?: string | undefined;
}

export const PersistentThreadTerminalPanel = memo(function PersistentThreadTerminalPanel({
  visible,
  threadRef,
  surface,
  launchContext,
  keybindings,
  onAddTerminalContext,
  onSplitTerminal,
  onSplitTerminalVertical,
  onNewTerminal,
  onActiveTerminalChange,
  onCloseTerminal,
  splitShortcutLabel,
  splitVerticalShortcutLabel,
  newShortcutLabel,
  closeShortcutLabel,
}: PersistentThreadTerminalPanelProps) {
  const messages = useMessages();
  const serverThread = useQuietThreadShell(threadRef);
  const draftThread = useComposerDraftStore((store) => store.getDraftThreadByRef(threadRef));
  const projectRef = serverThread
    ? scopeProjectRef(serverThread.environmentId, serverThread.projectId)
    : draftThread
      ? scopeProjectRef(draftThread.environmentId, draftThread.projectId)
      : null;
  const project = useProject(projectRef);
  const knownTerminalSessions = useKnownTerminalSessions({
    environmentId: threadRef.environmentId,
    threadId: threadRef.threadId,
  });
  const threadWorktreePath = serverThread?.worktreePath ?? draftThread?.worktreePath ?? null;
  const activeSummary =
    knownTerminalSessions?.find((session) => session.target.terminalId === surface.activeTerminalId)
      ?.state.summary ?? null;
  const defaultWorktreePath = resolveTerminalWorktreePath({
    launchContext,
    summary: null,
    threadWorktreePath,
  });
  const defaultCwd =
    launchContext?.cwd ??
    (project
      ? projectScriptCwd({
          project: { cwd: project.workspaceRoot },
          worktreePath: defaultWorktreePath,
        })
      : null);
  const defaultRuntimeEnv = project
    ? projectScriptRuntimeEnv({
        project: { cwd: project.workspaceRoot },
        worktreePath: defaultWorktreePath,
      })
    : {};
  const terminalLabelsById = new Map<string, string>();
  for (const terminalId of surface.terminalIds) {
    const summary =
      knownTerminalSessions?.find((session) => session.target.terminalId === terminalId)?.state
        .summary ?? null;
    terminalLabelsById.set(terminalId, resolveTerminalSessionLabel(terminalId, summary, messages));
  }
  const terminalLaunchLocationsById = useResolvedTerminalLaunchLocations({
    threadKey: scopedThreadKey(threadRef),
    openTerminalIds: project && defaultCwd ? surface.terminalIds : [],
    knownSessions: knownTerminalSessions ?? undefined,
    project,
    defaultLocation:
      project && defaultCwd
        ? { cwd: defaultCwd, worktreePath: defaultWorktreePath, runtimeEnv: defaultRuntimeEnv }
        : null,
  });
  const activeLocation = terminalLaunchLocationsById.get(surface.activeTerminalId);
  const worktreePath = activeSummary
    ? activeSummary.worktreePath
    : (activeLocation?.worktreePath ?? defaultWorktreePath);
  const cwd = activeSummary?.cwd ?? activeLocation?.cwd ?? defaultCwd;
  const runtimeEnv = activeSummary
    ? project
      ? projectScriptRuntimeEnv({
          project: { cwd: project.workspaceRoot },
          worktreePath: activeSummary.worktreePath,
        })
      : {}
    : (activeLocation?.runtimeEnv ?? defaultRuntimeEnv);

  if (!project || !cwd) return null;

  return (
    <ThreadTerminalDrawer
      mode="panel"
      visible={visible}
      threadRef={threadRef}
      threadId={threadRef.threadId}
      cwd={cwd}
      worktreePath={worktreePath}
      runtimeEnv={runtimeEnv}
      height={0}
      terminalIds={surface.terminalIds}
      activeTerminalId={surface.activeTerminalId}
      terminalGroups={[
        {
          id: surface.id,
          terminalIds: surface.terminalIds,
          ...(surface.splitDirection === "vertical" ? { splitDirection: "vertical" as const } : {}),
        },
      ]}
      activeTerminalGroupId={surface.id}
      onSplitTerminal={onSplitTerminal}
      onSplitTerminalVertical={onSplitTerminalVertical}
      onNewTerminal={onNewTerminal}
      splitShortcutLabel={splitShortcutLabel}
      splitVerticalShortcutLabel={splitVerticalShortcutLabel}
      newShortcutLabel={newShortcutLabel}
      closeShortcutLabel={closeShortcutLabel}
      onActiveTerminalChange={onActiveTerminalChange}
      onCloseTerminal={onCloseTerminal}
      onHeightChange={() => undefined}
      onAddTerminalContext={onAddTerminalContext}
      terminalLabelsById={terminalLabelsById}
      terminalLaunchLocationsById={terminalLaunchLocationsById}
      keybindings={keybindings}
    />
  );
});

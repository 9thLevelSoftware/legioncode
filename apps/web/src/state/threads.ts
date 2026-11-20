import { createChildThreadApprovalsAtom } from "@t3tools/client-runtime/state/thread-requests";
import { useAtomValue } from "@effect/atom-react";
import { enabledEnvironmentIds } from "@t3tools/client-runtime/state/connections";
import { arrayElementsEqual } from "@t3tools/client-runtime/state/entities";
import {
  createEnvironmentThreadDetailAtoms,
  createEnvironmentThreadShellAtoms,
  createEnvironmentThreadStateAtoms,
  EMPTY_ENVIRONMENT_THREAD_STATE,
  type EnvironmentThreadState,
  createOlderSettledThreadAtoms,
  createThreadEnvironmentAtoms,
  createThreadFamilyAtoms,
} from "@t3tools/client-runtime/state/threads";
import type { EnvironmentId, OrchestrationV2ThreadShell, ThreadId } from "@t3tools/contracts";
import * as Option from "effect/Option";
import { AsyncResult, Atom } from "effect/reactivity";

import { environmentCatalog } from "../connection/catalog";
import { connectionAtomRuntime } from "../connection/runtime";
import { serverEnvironment } from "./server";
import { environmentShell } from "./shell";

export const threadFamilies = createThreadFamilyAtoms(
  connectionAtomRuntime,
  environmentShell.stateAtom,
);
export const olderSettledThreads = createOlderSettledThreadAtoms(
  connectionAtomRuntime,
  environmentShell.stateAtom,
);
export const threadEnvironment = createThreadEnvironmentAtoms(
  connectionAtomRuntime,
  threadFamilies.snapshotAtom,
);
const environmentThreads = createEnvironmentThreadStateAtoms(connectionAtomRuntime);
export const environmentThreadDetails = createEnvironmentThreadDetailAtoms(
  environmentThreads.stateAtom,
);
export const environmentThreadShells = createEnvironmentThreadShellAtoms({
  catalogValueAtom: environmentCatalog.catalogValueAtom,
  snapshotAtom: threadEnvironment.snapshotAtom,
  hiddenThreadTagsAtom: serverEnvironment.hiddenThreadTagsAtom,
});
export const childThreadApprovalsAtom = createChildThreadApprovalsAtom({
  childThreadInputsAtom: environmentThreadShells.childThreadInputsAtom,
  pendingRequestsAtom: environmentThreadDetails.pendingRequestsAtom,
});

const EMPTY_THREAD_STATE_ATOM = Atom.make(AsyncResult.success(EMPTY_ENVIRONMENT_THREAD_STATE)).pipe(
  Atom.withLabel("web-environment-thread:empty"),
);

export function useEnvironmentThread(
  environmentId: EnvironmentId | null,
  threadId: ThreadId | null,
): EnvironmentThreadState {
  const result = useAtomValue(
    environmentId !== null && threadId !== null
      ? environmentThreads.stateAtom(environmentId, threadId)
      : EMPTY_THREAD_STATE_ATOM,
  );
  const state = AsyncResult.getOrElse(result, () => EMPTY_ENVIRONMENT_THREAD_STATE);
  return state;
}

const isRunning = (status: string) =>
  status === "preparing" || status === "queued" || status === "starting" || status === "running";

type KeptThreads = ReadonlyMap<EnvironmentId, ReadonlySet<ThreadId>>;

interface KeptThreadObservation {
  readonly baselineLiveStopped: boolean;
  readonly baselineSequence: number;
  readonly baselineData: EnvironmentThreadState["data"];
  sawFreshLiveSignal: boolean;
}

const observationKey = (environmentId: EnvironmentId, threadId: ThreadId) =>
  `${environmentId}\u0000${threadId}`;

function hasRunningRun(state: EnvironmentThreadState): boolean {
  return Option.exists(state.data, (thread) => thread.runs.some((run) => isRunning(run.status)));
}

function createInitialObservation<E>(
  result: AsyncResult.AsyncResult<EnvironmentThreadState, E>,
): KeptThreadObservation {
  if (!AsyncResult.isSuccess(result)) {
    return {
      baselineLiveStopped: false,
      baselineSequence: 0,
      baselineData: Option.none(),
      sawFreshLiveSignal: false,
    };
  }
  const { status, data, sequence } = result.value;
  const running = hasRunningRun(result.value);
  return {
    baselineLiveStopped: status === "live" && !running,
    baselineSequence: sequence,
    baselineData: data,
    sawFreshLiveSignal: status === "synchronizing" || running,
  };
}

function recordLiveObservation<E>(
  obs: KeptThreadObservation,
  result: AsyncResult.AsyncResult<EnvironmentThreadState, E>,
): void {
  if (obs.sawFreshLiveSignal || !AsyncResult.isSuccess(result)) return;
  const { status, data, sequence } = result.value;
  if (
    status === "synchronizing" ||
    hasRunningRun(result.value) ||
    (!obs.baselineLiveStopped && status === "live") ||
    (status === "live" &&
      (sequence > obs.baselineSequence ||
        Option.getOrNull(data) !== Option.getOrNull(obs.baselineData)))
  ) {
    obs.sawFreshLiveSignal = true;
  }
}

function isDetailDone<E>(
  obs: KeptThreadObservation,
  result: AsyncResult.AsyncResult<EnvironmentThreadState, E>,
): boolean {
  if (!AsyncResult.isSuccess(result)) return true;
  const { status, error } = result.value;
  if (status === "deleted" || Option.isSome(error)) return true;
  return status === "live" && !hasRunningRun(result.value) && obs.sawFreshLiveSignal;
}

/** Keeps stopped threads mounted until their independent detail stream is live and also reports stopped. */
export function createRunningThreadKeepAliveAtom<E>(input: {
  readonly environmentIdsAtom: Atom.Atom<ReadonlyArray<EnvironmentId>>;
  readonly threadsAtom: (
    environmentId: EnvironmentId,
  ) => Atom.Atom<ReadonlyArray<Pick<OrchestrationV2ThreadShell, "id" | "status">>>;
  readonly stateAtom: (
    environmentId: EnvironmentId,
    threadId: ThreadId,
  ) => Atom.Atom<AsyncResult.AsyncResult<EnvironmentThreadState, E>>;
}) {
  const runningThreadIdsAtom = Atom.family((environmentId: EnvironmentId) => {
    let previous: ReadonlyArray<ThreadId> = [];
    return Atom.make((get) => {
      const running = get(input.threadsAtom(environmentId)).flatMap((thread) =>
        isRunning(thread.status) ? [thread.id] : [],
      );
      if (arrayElementsEqual(previous, running)) return previous;
      previous = running;
      return running;
    }).pipe(Atom.withLabel(`web-running-thread-ids:${environmentId}`));
  });

  const observations = new Map<string, KeptThreadObservation>();

  return Atom.make((get): KeptThreads => {
    const previous = Option.getOrUndefined(get.self<KeptThreads>());
    const kept = new Map<EnvironmentId, ReadonlySet<ThreadId>>();
    const activeKeys = new Set<string>();
    for (const environmentId of get(input.environmentIdsAtom)) {
      const runningIds = new Set(get(runningThreadIdsAtom(environmentId)));
      const threadIds = new Set<ThreadId>();
      const watch = (threadId: ThreadId, currentlyRunning: boolean) => {
        const key = observationKey(environmentId, threadId);
        const stateAtom = input.stateAtom(environmentId, threadId);
        if (currentlyRunning) {
          get.mount(stateAtom);
        }
        const current = get.once(stateAtom);
        let obs = observations.get(key);
        if (obs === undefined) {
          obs = createInitialObservation(current);
          observations.set(key, obs);
        } else {
          recordLiveObservation(obs, current);
        }
        if (!currentlyRunning && isDetailDone(obs, current)) return;
        threadIds.add(threadId);
        activeKeys.add(key);
        if (!currentlyRunning) {
          get.mount(stateAtom);
        }
        const activeObs = obs;
        get.subscribe(stateAtom, (state) => {
          recordLiveObservation(activeObs, state);
          if (!currentlyRunning && isDetailDone(activeObs, state)) get.refreshSelf();
        });
      };
      for (const threadId of runningIds) watch(threadId, true);
      for (const threadId of previous?.get(environmentId) ?? []) {
        if (runningIds.has(threadId)) continue;
        // A get dependency would hold stopped streams open until another change rebuilds this atom.
        watch(threadId, false);
      }
      kept.set(environmentId, threadIds);
    }
    for (const key of observations.keys()) {
      if (!activeKeys.has(key)) observations.delete(key);
    }
    return kept;
  }).pipe(Atom.withLabel("web-running-thread-keep-alive"));
}

export const runningThreadKeepAliveAtom = createRunningThreadKeepAliveAtom({
  environmentIdsAtom: Atom.map(environmentCatalog.catalogValueAtom, (catalog) => [
    ...enabledEnvironmentIds(catalog),
  ]),
  threadsAtom: environmentThreadShells.environmentThreadsAtom,
  stateAtom: environmentThreads.stateAtom,
});

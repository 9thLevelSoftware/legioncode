import { ConnectionCatalogDocument } from "@legioncode/client-runtime/platform";
import type { EnvironmentId } from "@legioncode/contracts";
import * as Effect from "effect/Effect";
import * as Option from "effect/Option";
import * as Schema from "effect/Schema";

import * as DesktopConnectionCatalogStore from "../../app/DesktopConnectionCatalogStore.ts";
import * as LoopbackTunnel from "../../preview/LoopbackTunnel.ts";
import * as IpcChannels from "../channels.ts";
import * as DesktopIpc from "../DesktopIpc.ts";

export const decodeConnectionCatalogDocument = Schema.decodeUnknownEffect(
  Schema.fromJsonString(ConnectionCatalogDocument),
);

const activeCatalogEnvironmentIds = (document: ConnectionCatalogDocument): Set<EnvironmentId> => {
  const disabled = new Set(document.disabledEnvironmentIds);
  const active = new Set<EnvironmentId>();
  for (const target of document.targets) {
    if (!disabled.has(target.environmentId)) active.add(target.environmentId);
  }
  for (const profile of document.profiles) {
    if (!disabled.has(profile.environmentId)) active.add(profile.environmentId);
  }
  return active;
};

export const getConnectionCatalog = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.GET_CONNECTION_CATALOG_CHANNEL,
  payload: Schema.Void,
  result: Schema.NullOr(Schema.String),
  handler: Effect.fn("desktop.ipc.connectionCatalog.get")(function* () {
    const store = yield* DesktopConnectionCatalogStore.DesktopConnectionCatalogStore;
    return Option.getOrNull(yield* store.get);
  }),
});

export const setConnectionCatalog = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.SET_CONNECTION_CATALOG_CHANNEL,
  payload: Schema.String,
  result: Schema.Boolean,
  handler: Effect.fn("desktop.ipc.connectionCatalog.set")(function* (catalog) {
    const store = yield* DesktopConnectionCatalogStore.DesktopConnectionCatalogStore;
    const tunnel = yield* LoopbackTunnel.LoopbackTunnel;
    const previousRaw = yield* store.get.pipe(Effect.orElseSucceed(() => Option.none<string>()));
    const saved = yield* store.set(catalog);
    if (!saved || Option.isNone(previousRaw)) return saved;
    const [previousDoc, nextDoc] = yield* Effect.all([
      decodeConnectionCatalogDocument(previousRaw.value).pipe(Effect.orElseSucceed(() => null)),
      decodeConnectionCatalogDocument(catalog).pipe(Effect.orElseSucceed(() => null)),
    ]);
    if (previousDoc === null || nextDoc === null) return saved;
    const nextActiveIds = activeCatalogEnvironmentIds(nextDoc);
    const removedIds = Array.from(activeCatalogEnvironmentIds(previousDoc)).filter(
      (environmentId) => !nextActiveIds.has(environmentId),
    );
    yield* Effect.forEach(
      removedIds,
      (environmentId) => tunnel.stopForwardsForEnvironment(environmentId),
      { discard: true },
    );
    return saved;
  }),
});

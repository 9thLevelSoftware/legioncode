import {
  bootstrapRemoteBearerSession,
  fetchRemoteSessionState,
  RemoteEnvironmentAuthUndeclaredStatusError,
  type RemoteEnvironmentAuthError,
} from "@legioncode/client-runtime/authorization";
import { fetchRemoteEnvironmentDescriptor } from "@legioncode/client-runtime/environment";
import { SshConnectionProfile, sshTargetKey } from "@legioncode/client-runtime/connection";
import {
  EnvironmentAuthInvalidError,
  DesktopDiscoveredSshHostSchema,
  DesktopSshBearerBootstrapInputSchema,
  DesktopSshBearerRequestInputSchema,
  DesktopSshEnvironmentEnsureInputSchema,
  DesktopSshEnvironmentEnsureResultSchema,
  DesktopSshEnvironmentTargetSchema,
  DesktopSshHttpBaseUrlInputSchema,
  DesktopSshPasswordPromptCancelledType,
  DesktopSshPasswordPromptResolutionInputSchema,
  ExecutionEnvironmentDescriptor,
  EnvironmentInternalError,
  EnvironmentOperationForbiddenError,
  EnvironmentRequestInvalidError,
  EnvironmentScopeRequiredError,
  AuthAccessTokenResult,
  AuthSessionState,
  type EnvironmentId,
} from "@legioncode/contracts";
import { SshHttpBridgeError } from "@legioncode/ssh/errors";
import { resolveLoopbackSshHttpBaseUrl } from "@legioncode/ssh/tunnel";
import * as Effect from "effect/Effect";
import * as Option from "effect/Option";
import * as Schema from "effect/Schema";

import * as DesktopConnectionCatalogStore from "../../app/DesktopConnectionCatalogStore.ts";
import * as LoopbackTunnel from "../../preview/LoopbackTunnel.ts";
import * as IpcChannels from "../channels.ts";
import * as DesktopIpc from "../DesktopIpc.ts";
import { decodeConnectionCatalogDocument } from "./connectionCatalog.ts";
import * as DesktopSshEnvironment from "../../ssh/DesktopSshEnvironment.ts";
import * as DesktopSshPasswordPrompts from "../../ssh/DesktopSshPasswordPrompts.ts";

const DesktopSshEnvironmentRequestOperation = Schema.Literals([
  "fetch-environment-descriptor",
  "bootstrap-bearer-session",
  "fetch-session-state",
]);
type DesktopSshEnvironmentRequestOperation = typeof DesktopSshEnvironmentRequestOperation.Type;

type DesktopSshEnvironmentRequestCause = RemoteEnvironmentAuthError | SshHttpBridgeError;

const isEnvironmentAuthInvalidError = Schema.is(EnvironmentAuthInvalidError);
const isEnvironmentInternalError = Schema.is(EnvironmentInternalError);
const isEnvironmentOperationForbiddenError = Schema.is(EnvironmentOperationForbiddenError);
const isEnvironmentRequestInvalidError = Schema.is(EnvironmentRequestInvalidError);
const isEnvironmentScopeRequiredError = Schema.is(EnvironmentScopeRequiredError);
const isRemoteEnvironmentAuthUndeclaredStatusError = Schema.is(
  RemoteEnvironmentAuthUndeclaredStatusError,
);
const isSshHttpBridgeError = Schema.is(SshHttpBridgeError);

function readSshHttpStatus(cause: DesktopSshEnvironmentRequestCause): number | null {
  if (isRemoteEnvironmentAuthUndeclaredStatusError(cause) || isSshHttpBridgeError(cause)) {
    return cause.status ?? null;
  }
  if (isEnvironmentRequestInvalidError(cause)) {
    return 400;
  }
  if (isEnvironmentAuthInvalidError(cause)) {
    return 401;
  }
  if (isEnvironmentScopeRequiredError(cause)) {
    return 403;
  }
  if (isEnvironmentOperationForbiddenError(cause)) {
    return 403;
  }
  if (isEnvironmentInternalError(cause)) {
    return 500;
  }
  return null;
}

export class DesktopSshEnvironmentRequestError extends Schema.TaggedError<DesktopSshEnvironmentRequestError>()(
  "DesktopSshEnvironmentRequestError",
  {
    operation: DesktopSshEnvironmentRequestOperation,
    cause: Schema.Defect(),
    sshHttpStatus: Schema.NullOr(Schema.Number),
  },
) {
  // The web client matches "[ssh_http:500]" to explain version-skewed SSH pairing.
  override get message() {
    const prefix = this.sshHttpStatus === null ? "" : `[ssh_http:${this.sshHttpStatus}] `;
    return `${prefix}SSH remote API request failed during ${this.operation}.`;
  }
}

const withLoopbackSshApi =
  <A, R>(
    operation: DesktopSshEnvironmentRequestOperation,
    use: (httpBaseUrl: string) => Effect.Effect<A, RemoteEnvironmentAuthError, R>,
  ) =>
  (httpBaseUrl: string): Effect.Effect<A, DesktopSshEnvironmentRequestError, R> =>
    resolveLoopbackSshHttpBaseUrl(httpBaseUrl).pipe(
      Effect.flatMap(use),
      Effect.mapError(
        (cause) =>
          new DesktopSshEnvironmentRequestError({
            operation,
            cause,
            sshHttpStatus: readSshHttpStatus(cause),
          }),
      ),
    );

export const discoverSshHosts = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.DISCOVER_SSH_HOSTS_CHANNEL,
  payload: Schema.Void,
  result: Schema.Array(DesktopDiscoveredSshHostSchema),
  handler: Effect.fn("desktop.ipc.sshEnvironment.discoverHosts")(function* () {
    const sshEnvironment = yield* DesktopSshEnvironment.DesktopSshEnvironment;
    return yield* sshEnvironment.discoverHosts();
  }),
});

export const resolveSshHost = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.RESOLVE_SSH_HOST_CHANNEL,
  payload: Schema.String,
  result: DesktopSshEnvironmentTargetSchema,
  handler: Effect.fn("desktop.ipc.sshEnvironment.resolveHost")(function* (alias) {
    const sshEnvironment = yield* DesktopSshEnvironment.DesktopSshEnvironment;
    return yield* sshEnvironment.resolveHost(alias);
  }),
});

export const ensureSshEnvironment = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.ENSURE_SSH_ENVIRONMENT_CHANNEL,
  payload: DesktopSshEnvironmentEnsureInputSchema,
  result: DesktopSshEnvironmentEnsureResultSchema,
  handler: Effect.fn("desktop.ipc.sshEnvironment.ensureEnvironment")(function* ({
    target,
    options,
  }) {
    const sshEnvironment = yield* DesktopSshEnvironment.DesktopSshEnvironment;
    return yield* sshEnvironment.ensureEnvironment(target, options).pipe(
      Effect.catchIf(DesktopSshEnvironment.isDesktopSshPasswordPromptCancellation, (error) =>
        Effect.succeed({
          type: DesktopSshPasswordPromptCancelledType,
          message: error.message,
        }),
      ),
    );
  }),
});

const isSshConnectionProfile = Schema.is(SshConnectionProfile);

export const disconnectSshEnvironment = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.DISCONNECT_SSH_ENVIRONMENT_CHANNEL,
  payload: DesktopSshEnvironmentTargetSchema,
  result: Schema.Void,
  handler: Effect.fn("desktop.ipc.sshEnvironment.disconnectEnvironment")(function* (target) {
    const sshEnvironment = yield* DesktopSshEnvironment.DesktopSshEnvironment;
    const store = yield* DesktopConnectionCatalogStore.DesktopConnectionCatalogStore;
    const tunnel = yield* LoopbackTunnel.LoopbackTunnel;
    const rawCatalog = yield* store.get.pipe(Effect.orElseSucceed(() => Option.none<string>()));
    const environmentIds = yield* Option.match(rawCatalog, {
      onNone: () => Effect.succeed<ReadonlyArray<EnvironmentId>>([]),
      onSome: (raw) =>
        decodeConnectionCatalogDocument(raw).pipe(
          Effect.map((doc) => {
            const targetKey = sshTargetKey(target);
            const ids = new Set<EnvironmentId>();
            for (const profile of doc.profiles) {
              if (isSshConnectionProfile(profile) && sshTargetKey(profile.target) === targetKey) {
                ids.add(profile.environmentId);
              }
            }
            return Array.from(ids);
          }),
          Effect.orElseSucceed((): ReadonlyArray<EnvironmentId> => []),
        ),
    });
    yield* sshEnvironment
      .disconnectEnvironment(target)
      .pipe(
        Effect.ensuring(
          Effect.forEach(
            environmentIds,
            (environmentId) => tunnel.stopForwardsForEnvironment(environmentId),
            { discard: true },
          ).pipe(Effect.ignore),
        ),
      );
  }),
});

export const fetchSshEnvironmentDescriptor = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.FETCH_SSH_ENVIRONMENT_DESCRIPTOR_CHANNEL,
  payload: DesktopSshHttpBaseUrlInputSchema,
  result: ExecutionEnvironmentDescriptor,
  handler: Effect.fn("desktop.ipc.sshEnvironment.fetchDescriptor")(function* ({ httpBaseUrl }) {
    return yield* withLoopbackSshApi("fetch-environment-descriptor", (resolvedHttpBaseUrl) =>
      fetchRemoteEnvironmentDescriptor({ httpBaseUrl: resolvedHttpBaseUrl }),
    )(httpBaseUrl);
  }),
});

export const bootstrapSshBearerSession = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.BOOTSTRAP_SSH_BEARER_SESSION_CHANNEL,
  payload: DesktopSshBearerBootstrapInputSchema,
  result: AuthAccessTokenResult,
  handler: Effect.fn("desktop.ipc.sshEnvironment.bootstrapBearerSession")(function* ({
    httpBaseUrl,
    credential,
  }) {
    return yield* withLoopbackSshApi("bootstrap-bearer-session", (resolvedHttpBaseUrl) =>
      bootstrapRemoteBearerSession({
        httpBaseUrl: resolvedHttpBaseUrl,
        credential,
      }),
    )(httpBaseUrl);
  }),
});

export const fetchSshSessionState = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.FETCH_SSH_SESSION_STATE_CHANNEL,
  payload: DesktopSshBearerRequestInputSchema,
  result: AuthSessionState,
  handler: Effect.fn("desktop.ipc.sshEnvironment.fetchSessionState")(function* ({
    httpBaseUrl,
    bearerToken,
  }) {
    return yield* withLoopbackSshApi("fetch-session-state", (resolvedHttpBaseUrl) =>
      fetchRemoteSessionState({
        httpBaseUrl: resolvedHttpBaseUrl,
        bearerToken,
      }),
    )(httpBaseUrl);
  }),
});

export const resolveSshPasswordPrompt = DesktopIpc.makeIpcMethod({
  channel: IpcChannels.RESOLVE_SSH_PASSWORD_PROMPT_CHANNEL,
  payload: DesktopSshPasswordPromptResolutionInputSchema,
  result: Schema.Void,
  handler: Effect.fn("desktop.ipc.sshEnvironment.resolvePasswordPrompt")(function* ({
    requestId,
    password,
  }) {
    const prompts = yield* DesktopSshPasswordPrompts.DesktopSshPasswordPrompts;
    yield* prompts.resolve({ requestId, password });
  }),
});

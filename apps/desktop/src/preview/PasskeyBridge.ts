import type { CreateCredentialResult, GetCredentialResult } from "electron-webauthn";

import { PASSKEY_CREATE_CHANNEL, PASSKEY_GET_CHANNEL } from "./GuestProtocol.ts";

export type PasskeyCeremonyResult =
  | Extract<CreateCredentialResult, { success: true }>
  | Extract<GetCredentialResult, { success: true }>
  | { readonly success: false; readonly error: string };

type PasskeyChannel = typeof PASSKEY_CREATE_CHANNEL | typeof PASSKEY_GET_CHANNEL;
// Credential Management Level 1 added `mediation` to creation requests.
type CreationOptions = CredentialCreationOptions & {
  readonly mediation?: CredentialMediationRequirement;
};
type InvokePasskeyCeremony = (channel: PasskeyChannel, publicKey: unknown) => Promise<unknown>;

type PasskeyBridgeApi = {
  readonly create: (publicKey: unknown) => Promise<unknown>;
  readonly get: (publicKey: unknown) => Promise<unknown>;
};

// Chromium drops unknown members; discard uncloneable values instead of failing the ceremony.
export const toCloneable = function toCloneable(value: unknown): unknown {
  if (value instanceof ArrayBuffer) return value.slice(0);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice();
  }
  if (Array.isArray(value)) return value.map(toCloneable);
  if (typeof value === "object" && value !== null) {
    return Object.fromEntries(
      Object.entries(value).flatMap(([key, member]) =>
        typeof member === "function" || member === undefined ? [] : [[key, toCloneable(member)]],
      ),
    );
  }
  return value;
};

/**
 * Keep this body self-contained (zero outer closures) so `.toString()` can run inside the
 * page's main world when `contextIsolation` separates preload from `navigator.credentials`.
 */
const installMainWorldPasskeyBridge = (
  bridge: PasskeyBridgeApi,
  cloneValue: (value: unknown) => unknown,
) => {
  if (typeof CredentialsContainer === "undefined" || typeof PublicKeyCredential === "undefined") {
    return;
  }
  const notAllowedMessage =
    "The operation either timed out or was not allowed. See: https://www.w3.org/TR/webauthn-2/#sctn-privacy-considerations-client.";
  const domExceptionNames = new Set([
    "AbortError",
    "InvalidStateError",
    "NotAllowedError",
    "NotSupportedError",
    "SecurityError",
  ]);
  const decodeBase64Url = (value: string) => {
    const base64 = value.replaceAll("-", "+").replaceAll("_", "/");
    const binary = atob(base64.padEnd(base64.length + ((4 - (base64.length % 4)) % 4), "="));
    return Uint8Array.from(binary, (character) => character.charCodeAt(0)).buffer;
  };
  const encodeBase64Url = (buffer: ArrayBuffer) =>
    btoa(String.fromCharCode(...new Uint8Array(buffer)))
      .replaceAll("+", "-")
      .replaceAll("/", "_")
      .replace(/=+$/u, "");
  const toCeremonyError = (name: string) =>
    name === "TypeError"
      ? new TypeError("The passkey request options are invalid.")
      : new DOMException(notAllowedMessage, domExceptionNames.has(name) ? name : "NotAllowedError");
  const defineValues = <T extends object>(target: T, values: Record<string, unknown>) => {
    for (const [key, value] of Object.entries(values)) {
      Object.defineProperty(target, key, { value, enumerable: true, configurable: true });
    }
    return target;
  };
  const mapPrfOutputs = (prf: {
    readonly enabled?: boolean;
    readonly results?: { readonly first?: string; readonly second?: string };
  }): AuthenticationExtensionsPRFOutputs => {
    const first = prf.results?.first;
    const second = prf.results?.second;
    return {
      ...(prf.enabled === undefined ? {} : { enabled: prf.enabled }),
      ...(first === undefined
        ? {}
        : {
            results: {
              first: decodeBase64Url(first),
              ...(second === undefined ? {} : { second: decodeBase64Url(second) }),
            },
          }),
    };
  };
  const mapExtensionResultsJson = (results: AuthenticationExtensionsClientOutputs) =>
    JSON.parse(
      JSON.stringify(results, (_key, value: unknown) =>
        value instanceof ArrayBuffer ? encodeBase64Url(value) : value,
      ),
    ) as unknown;
  const buildCredential = (input: {
    readonly id: string;
    readonly authenticatorAttachment: AuthenticatorAttachment | null;
    readonly response: AuthenticatorResponse;
    readonly responseJson: Record<string, unknown>;
    readonly extensionResults: AuthenticationExtensionsClientOutputs;
  }) =>
    defineValues(Object.create(PublicKeyCredential.prototype) as PublicKeyCredential, {
      id: input.id,
      rawId: decodeBase64Url(input.id),
      type: "public-key",
      authenticatorAttachment: input.authenticatorAttachment,
      response: input.response,
      getClientExtensionResults: () => input.extensionResults,
      toJSON: () => ({
        id: input.id,
        rawId: input.id,
        type: "public-key",
        ...(input.authenticatorAttachment === null
          ? {}
          : { authenticatorAttachment: input.authenticatorAttachment }),
        response: input.responseJson,
        clientExtensionResults: mapExtensionResultsJson(input.extensionResults),
      }),
    });
  const createResultToCredential = (
    data: Extract<CreateCredentialResult, { success: true }>["data"],
  ) => {
    const transports = [...data.transports];
    const publicKey = data.publicKey.length > 0 ? data.publicKey : null;
    const extensionResults: AuthenticationExtensionsClientOutputs = {
      ...(data.extensions.credProps ? { credProps: { ...data.extensions.credProps } } : {}),
      ...(data.extensions.prf ? { prf: mapPrfOutputs(data.extensions.prf) } : {}),
      ...(data.extensions.largeBlob ? { largeBlob: { ...data.extensions.largeBlob } } : {}),
    };
    return buildCredential({
      id: data.credentialId,
      authenticatorAttachment: null,
      response: defineValues(
        Object.create(
          AuthenticatorAttestationResponse.prototype,
        ) as AuthenticatorAttestationResponse,
        {
          clientDataJSON: decodeBase64Url(data.clientDataJSON),
          attestationObject: decodeBase64Url(data.attestationObject),
          getTransports: () => [...transports],
          getAuthenticatorData: () => decodeBase64Url(data.authData),
          getPublicKey: () => (publicKey === null ? null : decodeBase64Url(publicKey)),
          getPublicKeyAlgorithm: () => data.publicKeyAlgorithm,
        },
      ),
      responseJson: {
        clientDataJSON: data.clientDataJSON,
        attestationObject: data.attestationObject,
        authenticatorData: data.authData,
        transports,
        ...(publicKey === null ? {} : { publicKey }),
        publicKeyAlgorithm: data.publicKeyAlgorithm,
      },
      extensionResults,
    });
  };
  const getResultToCredential = (data: Extract<GetCredentialResult, { success: true }>["data"]) => {
    const userHandle = data.userHandle.length > 0 ? data.userHandle : null;
    const { prf, largeBlob } = data.extensions ?? {};
    const extensionResults: AuthenticationExtensionsClientOutputs = {
      ...(prf ? { prf: mapPrfOutputs(prf) } : {}),
      ...(largeBlob
        ? {
            largeBlob: {
              ...(largeBlob.blob === undefined ? {} : { blob: decodeBase64Url(largeBlob.blob) }),
              ...(largeBlob.written === undefined ? {} : { written: largeBlob.written }),
            },
          }
        : {}),
    };
    return buildCredential({
      id: data.credentialId,
      authenticatorAttachment: null,
      response: defineValues(
        Object.create(AuthenticatorAssertionResponse.prototype) as AuthenticatorAssertionResponse,
        {
          clientDataJSON: decodeBase64Url(data.clientDataJSON),
          authenticatorData: decodeBase64Url(data.authenticatorData),
          signature: decodeBase64Url(data.signature),
          userHandle: userHandle === null ? null : decodeBase64Url(userHandle),
        },
      ),
      responseJson: {
        clientDataJSON: data.clientDataJSON,
        authenticatorData: data.authenticatorData,
        signature: data.signature,
        ...(userHandle === null ? {} : { userHandle }),
      },
      extensionResults,
    });
  };
  const toAbortReason = (signal: AbortSignal) =>
    signal.reason ?? new DOMException("The operation was aborted.", "AbortError");
  const run = async (
    kind: "create" | "get",
    publicKey: unknown,
    signal: AbortSignal | undefined,
  ): Promise<PublicKeyCredential> => {
    if (signal?.aborted) throw toAbortReason(signal);
    const call = kind === "create" ? bridge.create : bridge.get;
    const ceremony = call(cloneValue(publicKey)).then(
      (value) => {
        const result = value as PasskeyCeremonyResult;
        if (!result.success) throw toCeremonyError(result.error);
        return "attestationObject" in result.data
          ? createResultToCredential(result.data)
          : getResultToCredential(result.data);
      },
      (error: unknown) => {
        throw error instanceof DOMException || error instanceof TypeError
          ? error
          : toCeremonyError("NotAllowedError");
      },
    );
    if (!signal) return ceremony;
    return new Promise((resolve, reject) => {
      const onAbort = () => reject(toAbortReason(signal));
      signal.addEventListener("abort", onAbort, { once: true });
      ceremony.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
    });
  };

  const container = CredentialsContainer.prototype;
  const nativeCreate = container.create;
  const nativeGet = container.get;
  const define = (target: object, key: string, value: unknown) =>
    Object.defineProperty(target, key, { value, configurable: true, writable: true });

  define(
    container,
    "create",
    function create(this: CredentialsContainer, options?: CreationOptions) {
      return options?.publicKey && options.mediation !== "conditional"
        ? run("create", options.publicKey, options.signal)
        : Reflect.apply(nativeCreate, this, [options]);
    },
  );
  define(
    container,
    "get",
    function get(this: CredentialsContainer, options?: CredentialRequestOptions) {
      return options?.publicKey && options.mediation !== "conditional"
        ? run("get", options.publicKey, options.signal)
        : Reflect.apply(nativeGet, this, [options]);
    },
  );

  define(PublicKeyCredential, "isUserVerifyingPlatformAuthenticatorAvailable", async () => true);
  define(PublicKeyCredential, "isConditionalMediationAvailable", async () => false);
  const nativeCapabilities = PublicKeyCredential.getClientCapabilities;
  if (typeof nativeCapabilities === "function") {
    define(PublicKeyCredential, "getClientCapabilities", async () => ({
      ...(await Reflect.apply(nativeCapabilities, PublicKeyCredential, [])),
      conditionalCreate: false,
      conditionalGet: false,
      hybridTransport: true,
      passkeyPlatformAuthenticator: true,
      userVerifyingPlatformAuthenticator: true,
    }));
  }
};

export function buildMainWorldPasskeyBridgeScript(bridgeKey: string): string {
  const keyLiteral = JSON.stringify(bridgeKey);
  return `(() => {
  const key = ${keyLiteral};
  const bridge = globalThis[key];
  try { Reflect.deleteProperty(globalThis, key); } catch {}
  if (!bridge) return;
  (${installMainWorldPasskeyBridge.toString()})(bridge, ${toCloneable.toString()});
})();`;
}

/** Keep conditional requests native so autofill and upgrades cannot open a modal sheet unprompted. */
export function installPasskeyBridge(invoke: InvokePasskeyCeremony) {
  installMainWorldPasskeyBridge(
    {
      create: (publicKey) => invoke(PASSKEY_CREATE_CHANNEL, publicKey),
      get: (publicKey) => invoke(PASSKEY_GET_CHANNEL, publicKey),
    },
    toCloneable,
  );
}

export interface ReadDesktopPrimaryBearerTokenOptions {
  readonly forceRefresh?: boolean;
  readonly invalidatedToken?: string;
}

let desktopBearerTokenPromise: Promise<string> | null = null;
let desktopBearerTokenValue: string | null = null;
let desktopBearerTokenGeneration = 0;
let refreshingInvalidatedToken: string | null = null;

function startBearerTokenFetch(
  bridge: NonNullable<Window["desktopBridge"]>,
  options?: { readonly forceRefresh?: boolean },
): Promise<string> {
  const generation = ++desktopBearerTokenGeneration;
  const request = options?.forceRefresh
    ? bridge.getLocalEnvironmentBearerToken({ forceRefresh: true })
    : bridge.getLocalEnvironmentBearerToken();
  const promise = request
    .then((token) => {
      if (generation === desktopBearerTokenGeneration) {
        desktopBearerTokenValue = token;
      }
      return token;
    })
    .catch((error) => {
      if (generation === desktopBearerTokenGeneration && desktopBearerTokenPromise === promise) {
        desktopBearerTokenPromise = null;
        desktopBearerTokenValue = null;
      }
      throw error;
    })
    .finally(() => {
      if (desktopBearerTokenPromise === promise) {
        refreshingInvalidatedToken = null;
      }
    });
  desktopBearerTokenPromise = promise;
  return promise;
}

export function clearDesktopPrimaryBearerTokenCache(): void {
  desktopBearerTokenGeneration += 1;
  desktopBearerTokenPromise = null;
  desktopBearerTokenValue = null;
  refreshingInvalidatedToken = null;
}

export function readDesktopPrimaryBearerToken(
  options?: ReadDesktopPrimaryBearerTokenOptions,
): Promise<string | null> {
  if (typeof window === "undefined") {
    return Promise.resolve(null);
  }
  const bridge = window.desktopBridge;
  if (!bridge) {
    return Promise.resolve(null);
  }

  if (options?.forceRefresh) {
    if (
      options.invalidatedToken !== undefined &&
      desktopBearerTokenValue !== null &&
      desktopBearerTokenValue !== options.invalidatedToken &&
      desktopBearerTokenPromise !== null
    ) {
      return desktopBearerTokenPromise;
    }
    if (
      options.invalidatedToken !== undefined &&
      refreshingInvalidatedToken === options.invalidatedToken &&
      desktopBearerTokenPromise !== null
    ) {
      return desktopBearerTokenPromise;
    }
    desktopBearerTokenPromise = null;
    desktopBearerTokenValue = null;
    refreshingInvalidatedToken = options.invalidatedToken ?? null;
    return startBearerTokenFetch(bridge, { forceRefresh: true });
  }

  desktopBearerTokenPromise ??= startBearerTokenFetch(bridge);
  return desktopBearerTokenPromise;
}

export function __resetDesktopPrimaryAuthForTests(): void {
  clearDesktopPrimaryBearerTokenCache();
}

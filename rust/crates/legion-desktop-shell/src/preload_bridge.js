(() => {
  if (typeof window === "undefined" || window.desktopBridge) return;
  const CFG = __LEGION_DESKTOP_BRIDGE_INIT__;

  let nextId = 1;
  let localEnvEnabled = Boolean(CFG.localEnvironmentEnabled);
  let fullscreenState = Boolean(CFG.windowFullscreenState);
  const pending = new Map();
  const listeners = new Map();

  function emit(event, ...args) {
    const set = listeners.get(event);
    if (!set) return;
    for (const fn of Array.from(set)) {
      try {
        fn(...args);
      } catch (err) {
        console.error("[desktopBridge] listener error for", event, err);
      }
    }
  }

  function on(event, listener) {
    if (typeof listener !== "function") return () => {};
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
    }
    set.add(listener);
    return () => {
      set.delete(listener);
      if (set.size === 0) listeners.delete(event);
    };
  }

  function receive(raw) {
    let msg = raw;
    if (typeof raw === "string") {
      try {
        msg = JSON.parse(raw);
      } catch {
        return;
      }
    }
    if (!msg || typeof msg !== "object") return;
    if (typeof msg.event === "string") {
      if (msg.event === "windowFullscreenState") {
        fullscreenState = Boolean(msg.payload);
      }
      const args = Array.isArray(msg.args) ? msg.args : msg.payload === undefined ? [] : [msg.payload];
      emit(msg.event, ...args);
      return;
    }
    if (msg.id === undefined || msg.id === null) return;
    const entry = pending.get(msg.id);
    if (!entry) return;
    pending.delete(msg.id);
    if (msg.ok) {
      entry.resolve(msg.result);
    } else {
      const errMsg = (msg.error && msg.error.message) || "Desktop IPC error";
      entry.reject(new Error(errMsg));
    }
  }

  window.__legionReceiveIpc = receive;
  window.__legionEmitDesktopEvent = (event, ...args) => {
    if (event === "windowFullscreenState") fullscreenState = Boolean(args[0]);
    emit(event, ...args);
  };

  function invoke(method, params) {
    if (!window.ipc || typeof window.ipc.postMessage !== "function") {
      return Promise.reject(new Error(`Desktop native IPC unavailable for ${method}`));
    }
    const id = nextId++;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      try {
        window.ipc.postMessage(
          JSON.stringify({ id, method, params: params === undefined ? null : params }),
        );
      } catch (err) {
        pending.delete(id);
        reject(err);
      }
    });
  }

  function base64ToBytes(b64) {
    const bin = atob(b64 || "");
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
    return out;
  }

  function bytesToBase64(bytes) {
    if (!bytes) return "";
    let bin = "";
    const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    for (let i = 0; i < arr.length; i += 1) bin += String.fromCharCode(arr[i]);
    return btoa(bin);
  }

  function syncWindowControlsInset() {
    if (typeof document === "undefined" || !document.documentElement) return;
    const inset = CFG.clientPlatform === "darwin" && !fullscreenState ? "72px" : "0px";
    document.documentElement.style.setProperty("--desktop-window-controls-inset", inset);
  }

  if (typeof document !== "undefined") {
    syncWindowControlsInset();
    document.addEventListener("DOMContentLoaded", syncWindowControlsInset);
    document.addEventListener("fullscreenchange", () => {
      const next = Boolean(document.fullscreenElement);
      if (next !== fullscreenState) {
        fullscreenState = next;
        syncWindowControlsInset();
        emit("windowFullscreenState", fullscreenState);
      }
    });
    if (!CFG.primaryPasteEnabled) {
      window.addEventListener("mousedown", (e) => {
        if (e.button === 1) e.preventDefault();
      }, true);
    }
  }

  const noop = (method) => (arg) => invoke(`desktop.bridge.${method}`, arg);
  const previewVoid = (sub) => (...args) =>
    invoke(`desktop.bridge.preview.${sub}`, args.length <= 1 ? args[0] : { args });

  window.desktopBridge = {
    getAppBranding: () => CFG.branding,
    getRelayTelemetryEnabled: () => CFG.relayTelemetryEnabled,
    getPathForFile: (file) => (file && (file.path || file.name)) || "",
    getClientPlatform: () => CFG.clientPlatform,
    restartAudioService: () => invoke("desktop.bridge.restartAudioService"),
    setNotificationBadge: (badge) => invoke("desktop.bridge.setNotificationBadge", badge),
    onNotificationBadgeClear: (listener) => on("notificationBadgeClear", listener),
    onTrackpadScrollBegin: (listener) => on("trackpadScrollBegin", listener),
    onTrackpadScrollEnd: (listener) => on("trackpadScrollEnd", listener),
    revealWindow: () => invoke("desktop.bridge.revealWindow"),
    getSystemLocale: () => CFG.systemLocale,
    getPreferredSystemLanguages: () => CFG.preferredSystemLanguages,
    getPrimaryPasteEnabled: () => CFG.primaryPasteEnabled,
    getLocalEnvironmentBootstraps: () => CFG.localEnvironmentBootstraps,
    getLocalEnvironmentEnabled: () => localEnvEnabled,
    setLocalEnvironmentEnabled: async (enabled) => {
      localEnvEnabled = Boolean(enabled);
      await invoke("desktop.bridge.setLocalEnvironmentEnabled", { enabled: localEnvEnabled });
    },
    getHostEnvironmentId: () => invoke("desktop.bridge.getHostEnvironmentId"),
    getLocalEnvironmentBearerToken: (options) =>
      invoke("desktop.bridge.getLocalEnvironmentBearerToken", options),
    getClientSettings: () => invoke("desktop.bridge.getClientSettings"),
    setClientSettings: (settings) => invoke("desktop.bridge.setClientSettings", settings),
    getConnectionCatalog: () => invoke("desktop.bridge.getConnectionCatalog"),
    setConnectionCatalog: (catalog) => invoke("desktop.bridge.setConnectionCatalog", catalog),
    discoverSshHosts: () => invoke("desktop.bridge.discoverSshHosts"),
    resolveSshHost: (alias) => invoke("desktop.bridge.resolveSshHost", { alias }),
    requestSnapShotPermissions: (includeAccessibility) =>
      invoke("desktop.bridge.requestSnapShotPermissions", { includeAccessibility }),
    getSnapShotState: () => invoke("desktop.bridge.getSnapShotState"),
    setupSnapShot: (action) => invoke("desktop.bridge.setupSnapShot", { action }),
    previewSnapShotConfig: noop("previewSnapShotConfig"),
    applySnapShotConfig: (previewId) =>
      invoke("desktop.bridge.applySnapShotConfig", { previewId }),
    checkSnapShotShortcut: noop("checkSnapShotShortcut"),
    setSnapShotShortcutSuppressed: (suppressed) =>
      invoke("desktop.bridge.setSnapShotShortcutSuppressed", { suppressed }),
    listPendingSnapShots: () => invoke("desktop.bridge.listPendingSnapShots"),
    readSnapShot: (id) => invoke("desktop.bridge.readSnapShot", { id }),
    setSnapShotAnimationDestination: noop("setSnapShotAnimationDestination"),
    dismissSnapShotAnimation: (id) =>
      invoke("desktop.bridge.dismissSnapShotAnimation", { id }),
    acknowledgeSnapShot: (id) => invoke("desktop.bridge.acknowledgeSnapShot", { id }),
    ensureSshEnvironment: (target, options) =>
      invoke("desktop.bridge.ensureSshEnvironment", { target, options }),
    disconnectSshEnvironment: noop("disconnectSshEnvironment"),
    fetchSshEnvironmentDescriptor: (httpBaseUrl) =>
      invoke("desktop.bridge.fetchSshEnvironmentDescriptor", { httpBaseUrl }),
    bootstrapSshBearerSession: (httpBaseUrl, credential) =>
      invoke("desktop.bridge.bootstrapSshBearerSession", { httpBaseUrl, credential }),
    fetchSshSessionState: (httpBaseUrl, bearerToken) =>
      invoke("desktop.bridge.fetchSshSessionState", { httpBaseUrl, bearerToken }),
    onSshPasswordPrompt: (listener) => on("sshPasswordPrompt", listener),
    onSshPasswordPromptSettled: (listener) => on("sshPasswordPromptSettled", listener),
    resolveSshPasswordPrompt: (requestId, password) =>
      invoke("desktop.bridge.resolveSshPasswordPrompt", { requestId, password }),
    getServerExposureState: () => invoke("desktop.bridge.getServerExposureState"),
    setServerExposureMode: (mode) => invoke("desktop.bridge.setServerExposureMode", { mode }),
    setTailscaleServeEnabled: noop("setTailscaleServeEnabled"),
    getAdvertisedEndpoints: () => invoke("desktop.bridge.getAdvertisedEndpoints"),
    getWslState: () => invoke("desktop.bridge.getWslState"),
    setWslBackendEnabled: (enabled) => invoke("desktop.bridge.setWslBackendEnabled", { enabled }),
    setWslDistro: (distro) => invoke("desktop.bridge.setWslDistro", { distro }),
    setWslOnly: (enabled) => invoke("desktop.bridge.setWslOnly", { enabled }),
    pickFolder: (options) => invoke("desktop.bridge.pickFolder", options),
    toLocalBackendPath: (backendId, windowsPath) =>
      invoke("desktop.bridge.toLocalBackendPath", { backendId, windowsPath }),
    pickProjectFavicon: (initialPath) =>
      invoke("desktop.bridge.pickProjectFavicon", { initialPath }),
    pickThemeFiles: () => invoke("desktop.bridge.pickThemeFiles"),
    setTheme: (theme) => invoke("desktop.bridge.setTheme", { theme }),
    showContextMenu: (items, position, source) =>
      invoke("desktop.bridge.showContextMenu", { items, position, source }),
    receiveProviderAuthCallback: (authorizationUrl) =>
      invoke("desktop.bridge.receiveProviderAuthCallback", { authorizationUrl }),
    cancelProviderAuthCallback: (authorizationUrl) =>
      invoke("desktop.bridge.cancelProviderAuthCallback", { authorizationUrl }),
    startFileDrag: noop("startFileDrag"),
    openExternal: (url) => invoke("desktop.bridge.openExternal", { url }),
    readMedia: async (url) => {
      const res = await invoke("desktop.bridge.readMedia", { url });
      if (!res) return null;
      return {
        status: res.status,
        mimeType: res.mimeType,
        data: base64ToBytes(res.dataBase64),
      };
    },
    openSystemSettings: (pane) => invoke("desktop.bridge.openSystemSettings", { pane }),
    checkSystemPermission: (pane) => invoke("desktop.bridge.checkSystemPermission", { pane }),
    appleSpeech: {
      available: () => invoke("desktop.appleSpeech.available"),
      supportedLocales: () => invoke("desktop.appleSpeech.supportedLocales"),
      prepare: (locale) => invoke("desktop.appleSpeech.prepare", locale),
      transcribe: (input) => {
        const rawPcm = input && input.pcm;
        const pcm = ArrayBuffer.isView(rawPcm)
          ? Array.from(new Uint8Array(rawPcm.buffer, rawPcm.byteOffset, rawPcm.byteLength))
          : Array.isArray(rawPcm)
            ? rawPcm
            : [];
        return invoke("desktop.appleSpeech.transcribe", {
          locale: input && input.locale,
          pcm,
        });
      },
    },
    probeRemoteEditors: () => invoke("desktop.bridge.probeRemoteEditors"),
    pasteAsText: () => invoke("desktop.bridge.pasteAsText"),
    onMenuAction: (listener) => on("menuAction", listener),
    onSnapShotEvent: (listener) => on("snapShotEvent", listener),
    onQuitShortcut: (listener) => on("quitShortcut", listener),
    getWindowFullscreenState: () => fullscreenState,
    onWindowFullscreenStateChange: (listener) => on("windowFullscreenState", listener),
    getWindowZoomFactor: () => CFG.windowZoomFactor,
    getUpdateState: () => invoke("desktop.bridge.getUpdateState"),
    setUpdateChannel: async (channel, minimumVersion) => {
      const state = await invoke("desktop.bridge.setUpdateChannel", { channel, minimumVersion });
      emit("updateState", state);
      return state;
    },
    checkForUpdate: async () => {
      const res = await invoke("desktop.bridge.checkForUpdate");
      if (res && res.state) emit("updateState", res.state);
      return res;
    },
    downloadUpdate: async () => {
      const res = await invoke("desktop.bridge.downloadUpdate");
      if (res && res.state) emit("updateState", res.state);
      return res;
    },
    installUpdate: (expectedVersion) =>
      invoke("desktop.bridge.installUpdate", { expectedVersion }),
    onUpdateState: (listener) => on("updateState", listener),
    cliCommand: {
      getState: () => invoke("desktop.bridge.cliCommand.getState"),
      install: () => invoke("desktop.bridge.cliCommand.install"),
      uninstall: () => invoke("desktop.bridge.cliCommand.uninstall"),
      addToPath: () => invoke("desktop.bridge.cliCommand.addToPath"),
    },
    webLinks: {
      setReady: (ready) => invoke("desktop.bridge.webLinks.setReady", { ready }),
      onOpen: (listener) => on("webLinkOpen", listener),
    },
    appActivation: {
      setReady: (ready) => invoke("desktop.bridge.appActivation.setReady", { ready }),
      complete: (response) => invoke("desktop.bridge.appActivation.complete", response),
      onRequest: (listener) => on("appActivationRequest", listener),
    },
    preview: {
      setForwardedShortcuts: previewVoid("setForwardedShortcuts"),
      createTab: (tabId, defaults) => invoke("desktop.bridge.preview.createTab", { tabId, defaults }),
      closeTab: (tabId) => invoke("desktop.bridge.preview.closeTab", { tabId }),
      registerWebview: (tabId, webContentsId) =>
        invoke("desktop.bridge.preview.registerWebview", { tabId, webContentsId }),
      navigate: (tabId, url) => invoke("desktop.bridge.preview.navigate", { tabId, url }),
      goBack: (tabId) => invoke("desktop.bridge.preview.goBack", { tabId }),
      goForward: (tabId) => invoke("desktop.bridge.preview.goForward", { tabId }),
      refresh: (tabId) => invoke("desktop.bridge.preview.refresh", { tabId }),
      stopLoading: (tabId) => invoke("desktop.bridge.preview.stopLoading", { tabId }),
      zoomIn: (tabId) => invoke("desktop.bridge.preview.zoomIn", { tabId }),
      zoomOut: (tabId) => invoke("desktop.bridge.preview.zoomOut", { tabId }),
      resetZoom: (tabId) => invoke("desktop.bridge.preview.resetZoom", { tabId }),
      setZoomFactor: (tabId, zoomFactor) =>
        invoke("desktop.bridge.preview.setZoomFactor", { tabId, zoomFactor }),
      hardReload: (tabId) => invoke("desktop.bridge.preview.hardReload", { tabId }),
      setColorScheme: (tabId, colorScheme) =>
        invoke("desktop.bridge.preview.setColorScheme", { tabId, colorScheme }),
      setAudioMuted: (tabId, audioMuted) =>
        invoke("desktop.bridge.preview.setAudioMuted", { tabId, audioMuted }),
      findInPage: (tabId, text, options) =>
        invoke("desktop.bridge.preview.findInPage", { tabId, text, options }),
      stopFindInPage: (tabId) => invoke("desktop.bridge.preview.stopFindInPage", { tabId }),
      setShown: (tabId, shown) => invoke("desktop.bridge.preview.setShown", { tabId, shown }),
      setCaptureDrawn: (tabId, drawn) =>
        invoke("desktop.bridge.preview.setCaptureDrawn", { tabId, drawn }),
      openDevTools: (tabId) => invoke("desktop.bridge.preview.openDevTools", { tabId }),
      clearCookies: (environmentId, profileId) =>
        invoke("desktop.bridge.preview.clearCookies", { environmentId, profileId }),
      clearCache: (environmentId, profileId) =>
        invoke("desktop.bridge.preview.clearCache", { environmentId, profileId }),
      getPreviewConfig: (environmentId, profileId, tunnelLoopback) =>
        invoke("desktop.bridge.preview.getPreviewConfig", { environmentId, profileId, tunnelLoopback }),
      onTunnelAccessRequest: (listener) => on("previewTunnelAccessRequest", listener),
      resolveTunnelAccess: (requestId, url) =>
        invoke("desktop.bridge.preview.resolveTunnelAccess", { requestId, url }),
      forwardPort: (environmentId, remotePort) =>
        invoke("desktop.bridge.preview.forwardPort", { environmentId, remotePort }),
      stopPortForward: (environmentId, remotePort) =>
        invoke("desktop.bridge.preview.stopPortForward", { environmentId, remotePort }),
      listPortForwards: () => invoke("desktop.bridge.preview.listPortForwards"),
      sharesLoopback: (environmentId) =>
        invoke("desktop.bridge.preview.sharesLoopback", { environmentId }),
      onPortForwards: (listener) => on("previewPortForwards", listener),
      listBrowserImportSources: () => invoke("desktop.bridge.preview.listBrowserImportSources"),
      importBrowserCookies: (input) =>
        invoke("desktop.bridge.preview.importBrowserCookies", input),
      setAnnotationTheme: (theme) => invoke("desktop.bridge.preview.setAnnotationTheme", { theme }),
      setAnnotationSendEnabled: (tabId, enabled) =>
        invoke("desktop.bridge.preview.setAnnotationSendEnabled", { tabId, enabled }),
      annotationVoice: {
        onEvent: (listener) => on("previewAnnotationVoiceEvent", listener),
        update: (state) => invoke("desktop.bridge.preview.annotationVoice.update", state),
      },
      pickElement: (tabId, voice) => invoke("desktop.bridge.preview.pickElement", { tabId, voice }),
      cancelPickElement: (tabId) => invoke("desktop.bridge.preview.cancelPickElement", { tabId }),
      captureScreenshot: (tabId) => invoke("desktop.bridge.preview.captureScreenshot", { tabId }),
      revealArtifact: (path) => invoke("desktop.bridge.preview.revealArtifact", { path }),
      copyArtifactToClipboard: (path) =>
        invoke("desktop.bridge.preview.copyArtifactToClipboard", { path }),
      pictureInPicture: {
        open: (tabId) => invoke("desktop.bridge.preview.pictureInPicture.open", { tabId }),
        close: (tabId) => invoke("desktop.bridge.preview.pictureInPicture.close", { tabId }),
      },
      recording: {
        onInput: (listener) => on("previewRecordingInput", listener),
        startScreencast: (tabId) =>
          invoke("desktop.bridge.preview.recording.startScreencast", { tabId }),
        stopScreencast: (tabId) =>
          invoke("desktop.bridge.preview.recording.stopScreencast", { tabId }),
        save: (tabId, mimeType, data) =>
          invoke("desktop.bridge.preview.recording.save", {
            tabId,
            mimeType,
            dataBase64: bytesToBase64(data),
          }),
      },
      onStateChange: (listener) => on("previewStateChange", listener),
      onPointerEvent: (listener) => on("previewPointerEvent", listener),
      onOpenLink: (listener) => on("previewOpenLink", listener),
      onFindResult: (listener) => on("previewFindResult", listener),
    },
  };
})();

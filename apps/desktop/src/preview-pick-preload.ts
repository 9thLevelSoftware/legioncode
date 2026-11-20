// @effect-diagnostics cryptoRandomUUID:off -- Sandboxed Electron preload avoids bundling Effect runtime utilities.
import { contextBridge, ipcRenderer, webFrame } from "electron";

import {
  PASSKEY_BRIDGE_ARGUMENT,
  PASSKEY_CREATE_CHANNEL,
  PASSKEY_GET_CHANNEL,
  PRIMARY_PASTE_DISABLED_ARGUMENT,
} from "./preview/GuestProtocol.ts";
import { buildMainWorldPasskeyBridgeScript, toCloneable } from "./preview/PasskeyBridge.ts";
import {
  buildMainWorldReactInspectScript,
  registerMainWorldElementInspector,
  type MainWorldElementInspectorResult,
} from "./preview/PickPreload.ts";

const INSPECT_ATTRIBUTE = "data-t3code-pick-inspect";

registerMainWorldElementInspector(async (element) => {
  const token = globalThis.crypto.randomUUID();
  element.setAttribute(INSPECT_ATTRIBUTE, token);
  try {
    return (await webFrame.executeJavaScript(
      buildMainWorldReactInspectScript(INSPECT_ATTRIBUTE, token),
    )) as MainWorldElementInspectorResult | null;
  } finally {
    element.removeAttribute(INSPECT_ATTRIBUTE);
  }
});

// Electron hands webPreferences.additionalArguments to sandboxed preloads in process.argv.
if (process.argv.includes(PASSKEY_BRIDGE_ARGUMENT)) {
  const bridgeKey = `__t3codePasskeyBridge_${globalThis.crypto.randomUUID().replaceAll("-", "")}`;
  contextBridge.exposeInMainWorld(bridgeKey, {
    create: (publicKey: unknown) =>
      ipcRenderer.invoke(PASSKEY_CREATE_CHANNEL, toCloneable(publicKey)),
    get: (publicKey: unknown) => ipcRenderer.invoke(PASSKEY_GET_CHANNEL, toCloneable(publicKey)),
  });
  void webFrame.executeJavaScript(buildMainWorldPasskeyBridgeScript(bridgeKey));
}

if (process.argv.includes(PRIMARY_PASTE_DISABLED_ARGUMENT)) {
  // Same guard as the app preload: Blink pastes PRIMARY on an uncancelled middle mouseup.
  window.addEventListener(
    "mouseup",
    (event) => {
      if (event.button === 1) event.preventDefault();
    },
    true,
  );
}

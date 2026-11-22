import { expect, it, vi } from "vite-plus/test";

import { ELEMENT_PICKED_CHANNEL, START_PICK_CHANNEL } from "./GuestProtocol.ts";
import {
  buildMainWorldReactInspectScript,
  registerMainWorldElementInspector,
} from "./PickPreload.ts";

const ipcListeners = vi.hoisted(() => new Map<string, Array<(...args: unknown[]) => void>>());
const ipcSend = vi.hoisted(() => ({ calls: [] as unknown[][] }));

vi.mock("electron", () => ({
  ipcRenderer: {
    on: (channel: string, listener: (...args: unknown[]) => void) => {
      ipcListeners.set(channel, [...(ipcListeners.get(channel) ?? []), listener]);
    },
    off: () => {},
    send: (...args: unknown[]) => {
      ipcSend.calls.push(args);
    },
    invoke: async () => undefined,
  },
}));

vi.mock("react-grab/primitives", () => ({ getElementContext: async () => null }));

const startAnnotation = (): ShadowRoot => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  // jsdom has no popover API.
  HTMLElement.prototype.showPopover ??= () => {};
  HTMLElement.prototype.hidePopover ??= () => {};
  using attachShadow = vi.spyOn(Element.prototype, "attachShadow");
  for (const listener of ipcListeners.get(START_PICK_CHANNEL) ?? []) listener({}, undefined, false);
  const shadowRoot = attachShadow.mock.results[0]?.value;
  if (!(shadowRoot instanceof ShadowRoot)) throw new Error("annotation overlay did not start");
  return shadowRoot;
};

it("switches tools from the keyboard after a toolbar button is clicked", () => {
  const shadowRoot = startAnnotation();
  const region = Array.from(shadowRoot.querySelectorAll("button")).find(
    (button) => button.textContent === "Region",
  );
  if (!region) throw new Error("Region toolbar button not found");

  region.focus();
  region.click();
  expect(document.documentElement.getAttribute("data-legioncode-annotation-tool")).toBe("marquee");

  region.dispatchEvent(new KeyboardEvent("keydown", { key: "v", bubbles: true, composed: true }));

  expect(
    document.documentElement.getAttribute("data-legioncode-annotation-tool"),
    "V should switch to Select while focus is on a toolbar button",
  ).toBe("select");
});

it("keeps the tool while typing a tool key in the comment", () => {
  const shadowRoot = startAnnotation();
  const comment = shadowRoot.querySelector("textarea");
  if (!comment) throw new Error("comment textarea not found");

  comment.focus();
  comment.dispatchEvent(new KeyboardEvent("keydown", { key: "r", bubbles: true, composed: true }));

  expect(document.documentElement.getAttribute("data-legioncode-annotation-tool")).toBe("select");
});

it("keeps comment keystrokes away from the page's capture-phase shortcuts", () => {
  const pageShortcut = vi.fn((event: KeyboardEvent) => event.preventDefault());
  window.addEventListener("keydown", pageShortcut, { capture: true });
  const shadowRoot = startAnnotation();
  const comment = shadowRoot.querySelector("textarea");
  if (!comment) throw new Error("comment textarea not found");

  comment.focus();
  const space = new KeyboardEvent("keydown", {
    key: " ",
    code: "Space",
    bubbles: true,
    cancelable: true,
    composed: true,
  });
  comment.dispatchEvent(space);
  window.removeEventListener("keydown", pageShortcut, { capture: true });

  expect(pageShortcut).not.toHaveBeenCalled();
  expect(space.defaultPrevented).toBe(false);
});

it("keeps the annotation when Escape dismisses an IME candidate in the comment", () => {
  const shadowRoot = startAnnotation();
  const comment = shadowRoot.querySelector("textarea");
  if (!comment) throw new Error("comment textarea not found");

  comment.value = "draft";
  comment.focus();
  comment.dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Escape",
      isComposing: true,
      bubbles: true,
      cancelable: true,
      composed: true,
    }),
  );

  expect(shadowRoot.host.isConnected).toBe(true);
  expect(comment.value).toBe("draft");
});

it("ignores synthetic untrusted keyboard submission chords inside the comment box", () => {
  ipcSend.calls.length = 0;
  const card = document.body.appendChild(document.createElement("div"));
  using _rect = vi
    .spyOn(card, "getBoundingClientRect")
    .mockReturnValue(new DOMRect(40, 60, 200, 120));
  document.elementsFromPoint = () => [card];
  const shadowRoot = startAnnotation();
  const comment = shadowRoot.querySelector("textarea");
  if (!comment) throw new Error("comment textarea not found");

  card.dispatchEvent(
    new PointerEvent("pointerdown", { button: 0, clientX: 100, clientY: 100, bubbles: true }),
  );
  comment.value = "synthetic";
  comment.focus();
  comment.dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Enter",
      ctrlKey: true,
      metaKey: true,
      bubbles: true,
      cancelable: true,
      composed: true,
    }),
  );

  expect(ipcSend.calls.some(([channel]) => channel === ELEMENT_PICKED_CHANNEL)).toBe(false);
});

it.each([
  { scrolled: false, rect: { x: 40, y: 60, width: 200, height: 120 } },
  // The last rect would point at whatever now sits there, so no crop beats a wrong one.
  { scrolled: true, rect: { x: 0, y: 0, width: 0, height: 0 } },
])(
  "submits a replaced element's last rect unless the page scrolled ($scrolled)",
  async ({ scrolled, rect }) => {
    ipcSend.calls.length = 0;
    const card = document.body.appendChild(document.createElement("div"));
    const pickedRect = new DOMRect(40, 60, 200, 120);
    // Browsers report a detached element at 0x0 in the corner.
    using _rect = vi
      .spyOn(card, "getBoundingClientRect")
      .mockImplementation(() => (card.isConnected ? pickedRect : new DOMRect()));
    document.elementsFromPoint = () => [card];
    using listeners = vi.spyOn(HTMLButtonElement.prototype, "addEventListener");
    const shadowRoot = startAnnotation();

    card.dispatchEvent(
      new PointerEvent("pointerdown", { button: 0, clientX: 100, clientY: 100, bubbles: true }),
    );
    // A polling page re-renders while the comment is written and replaces the picked node.
    card.replaceWith(document.createElement("div"));
    if (scrolled) window.dispatchEvent(new Event("scroll"));
    const attach = Array.from(shadowRoot.querySelectorAll("button")).find(
      (button) => button.textContent === "Attach",
    );
    if (!attach) throw new Error("Attach button not found");
    attach.click();
    expect(attach.disabled).toBe(false);
    expect(ipcSend.calls.some(([channel]) => channel === ELEMENT_PICKED_CHANNEL)).toBe(false);
    const listenerIndex = listeners.mock.contexts.findIndex(
      (target, index) => target === attach && listeners.mock.calls[index]?.[0] === "click",
    );
    const listener = listeners.mock.calls[listenerIndex]?.[1];
    if (typeof listener !== "function") throw new Error("Attach click listener not found");
    // The DOM test environment has no native input source.
    listener.call(attach, { isTrusted: true } as MouseEvent);

    await vi.waitFor(() =>
      expect(ipcSend.calls.some(([channel]) => channel === ELEMENT_PICKED_CHANNEL)).toBe(true),
    );
    const [, annotation] =
      ipcSend.calls.find(([channel]) => channel === ELEMENT_PICKED_CHANNEL) ?? [];
    expect((annotation as { elements: Array<{ rect: unknown }> }).elements[0]?.rect).toEqual(rect);
  },
);

it("uses the registered main-world element inspector when isolated React lookup returns null", async () => {
  ipcSend.calls.length = 0;
  const card = document.body.appendChild(document.createElement("div"));
  (card as unknown as Record<string, unknown>)["__reactFiber$test"] = {
    type: Object.assign(() => null, { displayName: "IsolatedButton" }),
    _debugSource: { fileName: "Button.tsx", lineNumber: 42, columnNumber: 4 },
    return: null,
  };
  registerMainWorldElementInspector((element) => {
    element.setAttribute("data-legioncode-pick-inspect", "tok-1");
    try {
      const inspect = new Function(
        `return ${buildMainWorldReactInspectScript("data-legioncode-pick-inspect", "tok-1")}`,
      );
      return Promise.resolve(inspect() as ReturnType<typeof Object> | null) as never;
    } finally {
      element.removeAttribute("data-legioncode-pick-inspect");
    }
  });
  using _rect = vi
    .spyOn(card, "getBoundingClientRect")
    .mockReturnValue(new DOMRect(10, 20, 80, 40));
  document.elementsFromPoint = () => [card];
  using listeners = vi.spyOn(HTMLButtonElement.prototype, "addEventListener");
  const shadowRoot = startAnnotation();

  card.dispatchEvent(
    new PointerEvent("pointerdown", { button: 0, clientX: 20, clientY: 30, bubbles: true }),
  );
  const attach = Array.from(shadowRoot.querySelectorAll("button")).find(
    (button) => button.textContent === "Attach",
  );
  if (!attach) throw new Error("Attach button not found");
  const listenerIndex = listeners.mock.contexts.findIndex(
    (target, index) => target === attach && listeners.mock.calls[index]?.[0] === "click",
  );
  const listener = listeners.mock.calls[listenerIndex]?.[1];
  if (typeof listener !== "function") throw new Error("Attach click listener not found");
  listener.call(attach, { isTrusted: true } as MouseEvent);

  await vi.waitFor(() =>
    expect(ipcSend.calls.some(([channel]) => channel === ELEMENT_PICKED_CHANNEL)).toBe(true),
  );
  registerMainWorldElementInspector(null);
  const [, annotation] =
    ipcSend.calls.find(([channel]) => channel === ELEMENT_PICKED_CHANNEL) ?? [];
  const picked = (
    annotation as {
      elements: Array<{ element: { componentName: string | null; source: unknown } }>;
    }
  ).elements[0]?.element;
  expect(picked?.componentName).toBe("IsolatedButton");
  expect(picked?.source).toEqual({
    functionName: "IsolatedButton",
    fileName: "Button.tsx",
    lineNumber: 42,
    columnNumber: 4,
  });
});

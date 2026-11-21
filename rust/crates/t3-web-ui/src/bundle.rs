//! Deterministic static asset emitter (`index.html`, `styles.css`, `app.mjs`) for `t3-web-ui`.

use serde::{Deserialize, Serialize};

/// Deterministic static web bundle produced by [`emit_web_app_assets`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAppStaticAssets {
    /// Entry HTML5 document mounting `./app.mjs` and `./styles.css`.
    pub index_html: String,
    /// Responsive dark-mode workspace stylesheet.
    pub styles_css: String,
    /// Thin browser glue (`~120` lines) mounting `WasmWebApp` from `./t3_wasm_ffi.js`.
    pub app_mjs: String,
}

impl WebAppStaticAssets {
    /// Returns `(relative_path, contents)` pairs ready for static serving or disk emission.
    pub fn files(&self) -> [(&'static str, &str); 3] {
        [
            ("index.html", &self.index_html),
            ("styles.css", &self.styles_css),
            ("app.mjs", &self.app_mjs),
        ]
    }
}

/// Emits deterministic `index.html`, `styles.css`, and thin `app.mjs` browser glue.
pub fn emit_web_app_assets() -> WebAppStaticAssets {
    WebAppStaticAssets {
        index_html: INDEX_HTML.to_owned(),
        styles_css: STYLES_CSS.to_owned(),
        app_mjs: APP_MJS.to_owned(),
    }
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>T3 Code</title>
    <link rel="stylesheet" href="./styles.css" />
  </head>
  <body>
    <div id="app" data-testid="app-root"></div>
    <script type="module" src="./app.mjs"></script>
  </body>
</html>
"#;

const STYLES_CSS: &str = r":root {
  color-scheme: dark;
  --bg: #0f1117;
  --panel: #161b22;
  --border: #2b323d;
  --text: #e6edf3;
  --muted: #8b949e;
  --accent: #3b82f6;
  --add-bg: rgba(46, 160, 67, 0.16);
  --del-bg: rgba(248, 81, 73, 0.16);
  --word-hl: rgba(210, 153, 34, 0.36);
}
* { box-sizing: border-box; }
body { margin: 0; font-family: ui-sans-serif, system-ui, sans-serif; background: var(--bg); color: var(--text); }
.t3-shell { display: grid; grid-template-columns: 300px 1fr; min-height: 100vh; }
.t3-shell.is-sidebar-collapsed { grid-template-columns: 64px 1fr; }
.t3-sidebar { border-right: 1px solid var(--border); background: var(--panel); padding: 12px; display: flex; flex-direction: column; gap: 12px; }
.t3-brand-row, .t3-card-head, .t3-topbar, .t3-terminal-bar { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.t3-brand-row h1 { font-size: 1rem; margin: 0; }
.t3-card { border: 1px solid var(--border); border-radius: 8px; padding: 10px; display: flex; flex-direction: column; gap: 8px; }
.t3-main { display: flex; flex-direction: column; min-width: 0; height: 100vh; }
.t3-topbar { border-bottom: 1px solid var(--border); padding: 10px 14px; background: var(--panel); }
.t3-tabs { display: flex; gap: 6px; }
.t3-tab, .t3-btn, .t3-list-btn, .t3-diff-file-tab { background: #21262d; color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 6px 10px; cursor: pointer; font: inherit; }
.t3-tab.is-active, .t3-list-btn.is-active, .t3-diff-file-tab.is-active, .t3-btn-primary { background: var(--accent); border-color: var(--accent); color: #fff; }
.t3-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 4px; }
.t3-list-btn { width: 100%; text-align: left; }
.t3-input, .t3-select, .t3-composer-input, .t3-palette-input { width: 100%; background: var(--bg); color: var(--text); border: 1px solid var(--border); border-radius: 6px; padding: 6px 8px; font: inherit; }
.t3-panel { flex: 1; overflow: auto; padding: 14px; display: flex; flex-direction: column; gap: 10px; }
.t3-messages { display: flex; flex-direction: column; gap: 8px; }
.t3-msg { border: 1px solid var(--border); border-radius: 8px; padding: 10px; background: var(--panel); }
.t3-msg-head { display: flex; gap: 8px; font-size: 0.78rem; color: var(--muted); margin-bottom: 4px; }
.t3-msg-body { white-space: pre-wrap; word-break: break-word; }
.t3-composer { border-top: 1px solid var(--border); padding: 12px 14px; background: var(--panel); display: flex; flex-direction: column; gap: 8px; }
.t3-composer-form { display: flex; gap: 8px; align-items: flex-end; }
.t3-pills { display: flex; flex-wrap: wrap; gap: 6px; }
.t3-pill, .t3-chip { display: inline-flex; align-items: center; gap: 4px; font-size: 0.75rem; padding: 2px 8px; border-radius: 999px; border: 1px solid var(--border); background: #21262d; }
.t3-diff-lines, .t3-vt-screen { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.84rem; border: 1px solid var(--border); border-radius: 8px; overflow: auto; }
.t3-diff-line { display: grid; grid-template-columns: 44px 44px 1fr; padding: 1px 8px; white-space: pre; }
.t3-diff-add { background: var(--add-bg); }
.t3-diff-del { background: var(--del-bg); }
.t3-word-hl { background: var(--word-hl); border-radius: 2px; }
.t3-bold { font-weight: 700; }
.t3-italic { font-style: italic; }
.t3-underline { text-decoration: underline; }
.t3-strike { text-decoration: line-through; }
.t3-vt-screen { padding: 8px; line-height: 1.35; }
.t3-vt-row { white-space: pre; min-height: 1.35em; }
.t3-vt-cursor { outline: 2px solid var(--accent); }
.t3-palette-backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.65); display: grid; place-items: start center; padding-top: 10vh; z-index: 40; }
.t3-palette-card { width: min(640px, 92vw); background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 12px; display: flex; flex-direction: column; gap: 10px; }
.t3-palette-results { list-style: none; padding: 0; margin: 0; max-height: 320px; overflow: auto; display: flex; flex-direction: column; gap: 4px; }
.t3-palette-item { display: flex; justify-content: space-between; padding: 8px 10px; border-radius: 6px; cursor: pointer; }
.t3-palette-item.is-selected, .t3-palette-item:hover { background: #21262d; }
.t3-palette-mark { background: var(--word-hl); color: inherit; }
.t3-error { color: #f85149; font-size: 0.84rem; }
.t3-muted, .t3-empty-state { color: var(--muted); font-size: 0.9rem; }
@media (max-width: 768px) {
  .t3-shell { grid-template-columns: 1fr; }
  .t3-shell.is-sidebar-collapsed .t3-sidebar { display: none; }
}
";

const APP_MJS: &str = r#"import init, { WasmWebApp } from "./t3_wasm_ffi.js";

const STORAGE_KEY = "t3_session_token";
const root = document.getElementById("app");
let socket = null;
let engine = null;

function dispatch(action) {
  const active = document.activeElement;
  const focusId = active?.getAttribute?.("data-testid") || "";
  const sel = [active?.selectionStart, active?.selectionEnd];
  const out = JSON.parse(engine.dispatch_json(JSON.stringify(action)));
  root.innerHTML = out.html;
  if (focusId) {
    const next = root.querySelector(`[data-testid="${focusId}"]`);
    next?.focus?.();
    if (typeof sel[0] === "number") next?.setSelectionRange?.(sel[0], sel[1]);
  }
  for (const effect of out.effects || []) runEffect(effect);
}

async function runEffect(effect) {
  if (effect.type === "persistSessionToken") {
    if (effect.token) sessionStorage.setItem(STORAGE_KEY, effect.token);
    else sessionStorage.removeItem(STORAGE_KEY);
  } else if (effect.type === "focusSelector") {
    queueMicrotask(() => root.querySelector(effect.selector)?.focus());
  } else if (effect.type === "wsConnect") {
    connectSocket(effect.url);
  } else if (effect.type === "wsSendFrameParts" && socket?.readyState === WebSocket.OPEN) {
    for (const part of effect.parts) socket.send(part);
  } else if (effect.type === "httpFetch") {
    await executeHttpFetch(effect);
  }
}

async function executeHttpFetch(effect) {
  const headers = { "content-type": "application/json" };
  if (effect.bearerToken) headers.authorization = `Bearer ${effect.bearerToken}`;
  try {
    const res = await fetch(effect.url, {
      method: effect.method, headers, body: effect.body ?? undefined, credentials: "same-origin",
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) return dispatch({ type: "applyAuthError", message: data.message || `HTTP ${res.status}` });
    if (effect.url === "/api/auth/pair") {
      dispatch({
        type: "applyPairingSuccess",
        accessToken: data.accessToken || data.access_token || "",
        sessionId: data.sessionId ?? null, role: data.role ?? null,
        scopes: Array.isArray(data.scopes) ? data.scopes : [],
      });
    } else if (effect.url === "/api/projects") {
      dispatch({ type: "applyProjectsSnapshot", projects: data.projects || [] });
    } else if (effect.url === "/api/projects/mutate") {
      dispatch({ type: "applyProjectsSnapshot", projects: data.snapshot?.projects || [] });
    }
  } catch (err) {
    dispatch({ type: "applyAuthError", message: String(err) });
  }
}

function connectSocket(path) {
  socket?.close();
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  socket = new WebSocket(`${proto}//${location.host}${path}`);
  socket.addEventListener("open", () => dispatch({ type: "wsConnected", nowMs: Date.now() }));
  socket.addEventListener("close", () => dispatch({ type: "wsDisconnected", nowMs: Date.now() }));
  socket.addEventListener("message", (ev) => {
    if (typeof ev.data === "string") dispatch({ type: "receiveWsTextFrame", rawFrame: ev.data, nowMs: Date.now() });
  });
}

const CLICK_MAP = {
  "toggle-sidebar": () => ({ type: "toggleSidebar" }),
  "set-tab": (arg) => ({ type: "setRightTab", tab: arg }),
  "sign-out": () => ({ type: "signOut" }),
  "select-project": (arg) => ({ type: "selectProject", projectId: arg }),
  "select-thread": (arg) => ({ type: "selectThread", threadId: arg }),
  "new-thread": () => ({
    type: "createThread",
    projectId: root.querySelector("[data-testid='project-item'].is-active")?.dataset.arg || "proj-1",
    title: "New thread",
  }),
  "remove-pill": (arg) => ({ type: "removeContextPill", index: Number(arg) }),
  "select-diff-file": (arg) => ({ type: "selectDiffFile", index: Number(arg) }),
  "terminal-export": () => ({ type: "terminalExportSnapshot" }),
  "terminal-reset": () => ({ type: "terminalReset" }),
  "open-palette": () => ({ type: "openCommandPalette" }),
  "palette-exec": (arg) => ({ type: "executeCommandPaletteItem", index: Number(arg) }),
};

root.addEventListener("click", (ev) => {
  const el = ev.target.closest("[data-action]");
  const build = el && CLICK_MAP[el.dataset.action];
  if (build) dispatch(build(el.dataset.arg || ""));
});
root.addEventListener("submit", (ev) => {
  ev.preventDefault();
  const act = ev.target.dataset.action;
  if (act === "submit-pairing") dispatch({ type: "submitPairing" });
  else if (act === "create-project") {
    dispatch({ type: "createProject", workspaceRoot: root.querySelector("[data-testid='project-workspace-input']")?.value || "" });
  } else if (act === "send-message") dispatch({ type: "sendMessage" });
  else if (act === "terminal-paste") {
    dispatch({ type: "terminalPaste", rawText: root.querySelector("[data-testid='terminal-paste-input']")?.value || "" });
  }
});
root.addEventListener("input", (ev) => {
  const act = ev.target.dataset.action;
  if (act === "pairing-input") dispatch({ type: "setPairingInput", value: ev.target.value });
  else if (act === "project-ws-input") dispatch({ type: "setProjectDraft", workspaceRoot: ev.target.value });
  else if (act === "composer-input") dispatch({ type: "updateComposerDraft", text: ev.target.value });
  else if (act === "palette-input") dispatch({ type: "setCommandPaletteQuery", query: ev.target.value });
});
root.addEventListener("change", (ev) => {
  if (ev.target.dataset.action === "set-locale") dispatch({ type: "setLocale", language: ev.target.value });
});
root.addEventListener("paste", (ev) => {
  if (ev.target.dataset.action === "composer-input") {
    ev.preventDefault();
    dispatch({ type: "pasteIntoComposer", rawText: ev.clipboardData?.getData("text/plain") || "" });
  }
});
window.addEventListener("keydown", (ev) => {
  if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "k") {
    ev.preventDefault();
    dispatch({ type: "toggleCommandPalette" });
  } else if (ev.key === "Escape") dispatch({ type: "closeCommandPalette" });
});

await init();
engine = new WasmWebApp(navigator.language || "en", Date.now());
const savedToken = sessionStorage.getItem(STORAGE_KEY);
if (savedToken) dispatch({ type: "restoreSessionToken", token: savedToken });
else dispatch({ type: "setLocale", language: "system", systemLocales: navigator.languages || [] });
"#;

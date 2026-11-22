//! Pre-hydrated `window.desktopBridge` init-script JS contract and sidecar spawn tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use legion_desktop_shell::ipc::DesktopIpcRouter;
use legion_desktop_shell::native_dialogs::{
    MAX_THEME_FILE_BYTES, probe_remote_editors_in_path, read_picked_theme_files,
};
use legion_desktop_shell::preload_bridge::{
    build_bridge_init_config, render_desktop_bridge_init_script,
};
use legion_desktop_shell::sidecar::{
    RuntimeDiscoverySource, SidecarConfig, SidecarController, SidecarMode,
    desktop_serve_launch_args, generate_ephemeral_hex_token, mint_desktop_pairing_token,
    pick_unused_loopback_port,
};

static JS_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct JsTempDir {
    path: PathBuf,
}

impl JsTempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        let idx = JS_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{idx}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for JsTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn desktop_app_branding_resolves_dev_nightly_and_beta() {
    let dev = DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
        mode: SidecarMode::Development,
        app_version: "0.1.11".to_owned(),
        ..SidecarConfig::default()
    }));
    let dev_brand = dev.resolve_app_branding();
    assert_eq!(dev_brand.base_name, "LegionCode");
    assert_eq!(dev_brand.stage_label, "Dev");
    assert_eq!(dev_brand.display_name, "LegionCode (Dev)");

    let nightly = DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
        mode: SidecarMode::Production,
        app_version: "0.1.11-nightly.20261122".to_owned(),
        ..SidecarConfig::default()
    }));
    let nightly_brand = nightly.resolve_app_branding();
    assert_eq!(nightly_brand.stage_label, "Nightly");
    assert_eq!(nightly_brand.display_name, "LegionCode (Nightly)");

    let beta = DesktopIpcRouter::new(SidecarController::new(SidecarConfig {
        mode: SidecarMode::Production,
        app_version: "0.1.11".to_owned(),
        ..SidecarConfig::default()
    }));
    let beta_brand = beta.resolve_app_branding();
    assert_eq!(beta_brand.stage_label, "Beta");
    assert_eq!(beta_brand.display_name, "LegionCode (Beta)");
}

const BRIDGE_CONTRACT_RUNNER_JS: &str = r#"
const fs = require("node:fs");
const vm = require("node:vm");
const script = fs.readFileSync(process.argv[2], "utf8");
const contracts = fs.readFileSync(process.argv[3], "utf8");

function extractInterfaceKeys(src, name) {
  const start = src.indexOf(`export interface ${name} {`);
  if (start < 0) throw new Error(`Missing interface ${name}`);
  let depth = 0, bodyStart = -1, bodyEnd = -1;
  for (let i = start; i < src.length; i++) {
    if (src[i] === "{") { if (depth === 0) bodyStart = i + 1; depth++; }
    else if (src[i] === "}") { depth--; if (depth === 0) { bodyEnd = i; break; } }
  }
  const keys = [];
  let subDepth = 0, parenDepth = 0;
  for (const rawLine of src.slice(bodyStart, bodyEnd).split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("//") || line.startsWith("/*") || line.startsWith("*")) continue;
    if (subDepth === 0 && parenDepth === 0) {
      const m = /^([a-zA-Z0-9_]+)\??:\s*/.exec(line);
      if (m) keys.push(m[1]);
    }
    for (const ch of line) {
      if (ch === "{") subDepth++; else if (ch === "}") subDepth--;
      else if (ch === "(") parenDepth++; else if (ch === ")") parenDepth--;
    }
  }
  return keys;
}

const posted = [];
const sandbox = {
  window: { ipc: { postMessage: (msg) => posted.push(JSON.parse(msg)) }, addEventListener() {} },
  document: { documentElement: { style: { setProperty() {} } }, addEventListener() {} },
  atob: (s) => Buffer.from(s, "base64").toString("binary"),
  btoa: (s) => Buffer.from(s, "binary").toString("base64"),
  console, Uint8Array, Map, Set, Array, Promise, Error, JSON, Buffer,
};
vm.runInNewContext(script, sandbox);
const bridge = sandbox.window.desktopBridge;
if (!bridge) throw new Error("window.desktopBridge not installed");

for (const key of extractInterfaceKeys(contracts, "DesktopBridge")) {
  if (!(key in bridge)) throw new Error(`Missing DesktopBridge key: ${key}`);
}
for (const key of extractInterfaceKeys(contracts, "DesktopPreviewBridge")) {
  if (!(key in bridge.preview)) throw new Error(`Missing DesktopPreviewBridge key: preview.${key}`);
}
for (const k of ["getState", "install", "uninstall", "addToPath"]) {
  if (typeof bridge.cliCommand[k] !== "function") throw new Error(`Missing cliCommand.${k}`);
}
for (const k of ["setReady", "onOpen"]) {
  if (typeof bridge.webLinks[k] !== "function") throw new Error(`Missing webLinks.${k}`);
}
for (const k of ["setReady", "complete", "onRequest"]) {
  if (typeof bridge.appActivation[k] !== "function") throw new Error(`Missing appActivation.${k}`);
}
for (const k of ["onEvent", "update"]) {
  if (typeof bridge.preview.annotationVoice[k] !== "function") throw new Error(`Missing annotationVoice.${k}`);
}
for (const k of ["open", "close"]) {
  if (typeof bridge.preview.pictureInPicture[k] !== "function") throw new Error(`Missing pictureInPicture.${k}`);
}
for (const k of ["onInput", "startScreencast", "stopScreencast", "save"]) {
  if (typeof bridge.preview.recording[k] !== "function") throw new Error(`Missing recording.${k}`);
}

const brand = bridge.getAppBranding();
if (brand.baseName !== "LegionCode" || brand.stageLabel !== "Beta") throw new Error("Bad branding");
if (bridge.getPathForFile({ name: "notes.md" }) !== "notes.md") throw new Error("Bad getPathForFile");
const boots = bridge.getLocalEnvironmentBootstraps();
if (boots.length !== 1 || boots[0].id !== "primary") throw new Error("Bad bootstraps");

(async () => {
  const p = bridge.readMedia("http://127.0.0.1:41234/a.png");
  const req = posted.pop();
  if (!req || req.method !== "desktop.bridge.readMedia") throw new Error("Missing posted readMedia");
  sandbox.window.__legionReceiveIpc(JSON.stringify({
    id: req.id, ok: true, result: { status: 200, mimeType: "image/png", dataBase64: "AQID" },
  }));
  const media = await p;
  if (!(media.data instanceof Uint8Array) || media.data.length !== 3 || media.data[2] !== 3) {
    throw new Error("Bad decoded media bytes");
  }
  let seenEvent = null;
  const unsub = bridge.onUpdateState((s) => { seenEvent = s; });
  sandbox.window.__legionReceiveIpc(JSON.stringify({ event: "updateState", payload: { status: "idle" } }));
  if (!seenEvent || seenEvent.status !== "idle") throw new Error("Missing updateState event");
  unsub();
})().catch((e) => { console.error(e); process.exit(1); });
"#;

#[test]
fn preload_bridge_script_covers_all_contract_keys_and_executes_cleanly_in_js() -> Result<(), String>
{
    let cfg = build_bridge_init_config(
        false,
        "0.1.11",
        Some("http://127.0.0.1:41234".to_owned()),
        Some("ws://127.0.0.1:41234/ws".to_owned()),
        Some("tok</script>&\u{2028}secret".to_owned()),
    );
    let script = render_desktop_bridge_init_script(&cfg)?;
    assert!(!script.contains("tok</script>"));
    assert!(script.contains(r"tok\u003c/script\u003e\u0026\u2028secret"));

    let contracts_ipc = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packages/contracts/src/ipc.ts")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    assert!(contracts_ipc.is_file());

    let tmp = JsTempDir::new("legion-bridge-js")?;
    let js_path = tmp.path().join("init.js");
    let check_path = tmp.path().join("check.cjs");
    fs::write(&js_path, &script).map_err(|e| e.to_string())?;
    fs::write(&check_path, BRIDGE_CONTRACT_RUNNER_JS).map_err(|e| e.to_string())?;
    let out = Command::new("node")
        .arg(&check_path)
        .arg(&js_path)
        .arg(&contracts_ipc)
        .output()
        .map_err(|e| e.to_string())?;
    assert!(
        out.status.success(),
        "node JS bridge contract validation failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

#[test]
fn sidecar_discovery_supports_rust_bin_sibling_bin_and_isolated_home() -> Result<(), String> {
    let tmp = JsTempDir::new("legion-sidecar-managed")?;
    let bin_name = if cfg!(windows) {
        "legioncode.exe"
    } else {
        "legioncode"
    };
    let desktop_name = if cfg!(windows) {
        "legioncode-desktop.exe"
    } else {
        "legioncode-desktop"
    };

    let app_bin_dir = tmp.path().join("pkg").join("Contents").join("MacOS");
    let alt_bin_dir = tmp.path().join("pkg").join("Contents").join("bin");
    fs::create_dir_all(&app_bin_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&alt_bin_dir).map_err(|e| e.to_string())?;
    let host_exe = app_bin_dir.join(desktop_name);
    let sibling_parent_bin = alt_bin_dir.join(bin_name);
    let rust_bin = tmp.path().join("override-bin").join(bin_name);
    fs::create_dir_all(rust_bin.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    fs::write(&host_exe, b"desktop").map_err(|e| e.to_string())?;
    fs::write(&sibling_parent_bin, b"cli").map_err(|e| e.to_string())?;
    fs::write(&rust_bin, b"rust-bin").map_err(|e| e.to_string())?;

    let ctrl = SidecarController::default();
    let found_parent_bin =
        ctrl.discover_runtime_with_env_inputs(None, None, None, Some(&host_exe), false);
    assert!(found_parent_bin.found);
    assert_eq!(
        found_parent_bin.source,
        RuntimeDiscoverySource::SiblingBinary
    );
    assert_eq!(found_parent_bin.binary_path, sibling_parent_bin);

    let found_rust_bin = ctrl.discover_runtime_with_env_inputs(
        Some(&rust_bin.to_string_lossy()),
        None,
        None,
        Some(&host_exe),
        false,
    );
    assert!(found_rust_bin.found);
    assert_eq!(found_rust_bin.source, RuntimeDiscoverySource::RustBinEnv);
    assert_eq!(found_rust_bin.binary_path, rust_bin);

    let port = pick_unused_loopback_port()?;
    assert!(port > 0);
    let args = desktop_serve_launch_args(port, tmp.path());
    assert_eq!(args[0], "serve");
    assert_eq!(args[4], port.to_string());
    assert_eq!(generate_ephemeral_hex_token().len(), 32);

    let db_path = tmp.path().join("userdata/statev2.sqlite");
    let token = mint_desktop_pairing_token(&db_path, "Test Desktop")?;
    assert_ne!(token, "");
    assert!(db_path.is_file());

    let theme_path = tmp.path().join("big.json");
    let oversized_len = usize::try_from(MAX_THEME_FILE_BYTES).map_err(|e| e.to_string())? + 8;
    fs::write(&theme_path, vec![b'a'; oversized_len]).map_err(|e| e.to_string())?;
    let picked = read_picked_theme_files(&[theme_path.to_string_lossy().into_owned()])?;
    assert_eq!(picked[0].text, "");

    let fake_cursor = tmp.path().join("editors").join(if cfg!(windows) {
        "cursor.cmd"
    } else {
        "cursor"
    });
    fs::create_dir_all(fake_cursor.parent().ok_or("no editor parent")?)
        .map_err(|e| e.to_string())?;
    fs::write(&fake_cursor, b"").map_err(|e| e.to_string())?;
    assert_eq!(
        probe_remote_editors_in_path(fake_cursor.parent().map(Path::as_os_str)),
        vec!["cursor"]
    );
    Ok(())
}

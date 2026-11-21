//! Headless browser host discovery, `PreviewBrowserHost.ts` failure diagnostics, and [`PreviewBrowserRuntime`].

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use t3_process_ffi::env::SanitizedChildEnv;
use t3_process_ffi::pipes::PipeSetConfig;
use t3_process_ffi::supervisor::{JobProcessSupervisor, ProcessToken, SpawnSpec};
use tokio::sync::Mutex as TokioMutex;

use crate::error::ServerRuntimeError;
pub use crate::preview_browser_diag::{
    PreviewLaunchDiagnostic, diagnose_browser_launch_failure, discover_browser_executable,
};
use crate::preview_cdp::{CdpConnection, CdpKeyEventParams, CdpMouseEventParams};

/// Action dispatched against [`PreviewBrowserRuntime`] or `/api/preview/action`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum PreviewAction {
    /// Connect to explicit `ws://` CDP URL or spawn a local headless browser.
    #[serde(rename_all = "camelCase")]
    Connect {
        /// Explicit CDP WebSocket URL (optional; launches local browser when omitted).
        #[serde(default)]
        ws_url: Option<String>,
        /// Optional initial URL when creating a target.
        #[serde(default)]
        target_url: Option<String>,
    },
    /// Create a new browser tab/target and enable `Page`.
    CreateTarget {
        /// Target initial URL (`about:blank` if omitted).
        #[serde(default)]
        url: Option<String>,
    },
    /// Navigate the active page to `url`.
    Navigate {
        /// Destination URL.
        url: String,
    },
    /// Evaluate JavaScript expression on the active page.
    Evaluate {
        /// JavaScript expression string.
        expression: String,
    },
    /// Capture a PNG or JPEG base64 screenshot of the active page.
    Screenshot {
        /// Image format (`"png"` or `"jpeg"`).
        #[serde(default)]
        format: Option<String>,
    },
    /// Dispatch mouse input event.
    MouseEvent {
        /// CDP mouse event parameters.
        #[serde(flatten)]
        params: CdpMouseEventParams,
    },
    /// Dispatch keyboard input event.
    KeyEvent {
        /// CDP key event parameters.
        #[serde(flatten)]
        params: CdpKeyEventParams,
    },
    /// Set viewport metrics (`Emulation.setDeviceMetricsOverride`).
    #[serde(rename_all = "camelCase")]
    SetViewport {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Device pixel ratio (`1.0` default).
        #[serde(default = "default_scale")]
        device_scale_factor: f64,
        /// Whether to emulate mobile viewport.
        #[serde(default)]
        mobile: bool,
    },
    /// Close active CDP connection and kill any spawned browser child tree.
    Close,
}

const fn default_scale() -> f64 {
    1.0
}

struct BrowserChild {
    supervisor: JobProcessSupervisor,
    token: ProcessToken,
}

impl Drop for BrowserChild {
    fn drop(&mut self) {
        let _ = self.supervisor.terminate_child(self.token);
        let _ = self.supervisor.terminate_tree(1);
    }
}

/// Runtime managing a live CDP WebSocket session and optional supervised headless browser child.
#[derive(Clone, Default)]
pub struct PreviewBrowserRuntime {
    inner: Arc<TokioMutex<PreviewInner>>,
}

#[derive(Default)]
struct PreviewInner {
    cdp: Option<CdpConnection>,
    child: Option<BrowserChild>,
    ws_url: Option<String>,
}

impl PreviewBrowserRuntime {
    /// Construct an idle [`PreviewBrowserRuntime`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Connect to an explicit `ws://` CDP WebSocket URL (closing any previous session).
    pub async fn connect_ws(&self, ws_url: &str) -> Result<(), ServerRuntimeError> {
        self.close().await;
        let mut conn = CdpConnection::connect(ws_url).await?;
        let _ = conn.page_enable().await?;
        let mut guard = self.inner.lock().await;
        guard.cdp = Some(conn);
        guard.ws_url = Some(ws_url.to_owned());
        Ok(())
    }

    /// Spawn a local headless browser under [`JobProcessSupervisor`] and attach over CDP.
    pub async fn launch_local(
        &self,
        executable: Option<&Path>,
    ) -> Result<String, ServerRuntimeError> {
        self.close().await;
        let exe = discover_browser_executable(executable).ok_or_else(|| {
            ServerRuntimeError::NotFound("no headless Chromium/Edge executable found".to_owned())
        })?;
        let (child, ws_url) = tokio::task::spawn_blocking(move || spawn_chromium_and_wait_ws(&exe))
            .await
            .map_err(|e| {
                ServerRuntimeError::Process(format!("browser launch task failed: {e}"))
            })??;
        let mut conn = CdpConnection::connect(&ws_url).await?;
        let _ = conn.page_enable().await?;
        let mut guard = self.inner.lock().await;
        guard.child = Some(child);
        guard.cdp = Some(conn);
        guard.ws_url = Some(ws_url.clone());
        Ok(ws_url)
    }

    /// Execute one [`PreviewAction`] against the active CDP connection.
    pub async fn execute_action(&self, action: PreviewAction) -> Result<Value, ServerRuntimeError> {
        match action {
            PreviewAction::Connect { ws_url, target_url } => {
                if let Some(url) = ws_url {
                    self.connect_ws(&url).await?;
                } else {
                    let _ = self.launch_local(None).await?;
                }
                if let Some(t_url) = target_url {
                    let mut guard = self.inner.lock().await;
                    let cdp = guard.cdp.as_mut().ok_or_else(|| {
                        ServerRuntimeError::Conflict("no active CDP session".to_owned())
                    })?;
                    let target_id = cdp.create_target(&t_url).await?;
                    return Ok(json!({ "connected": true, "targetId": target_id }));
                }
                Ok(json!({ "connected": true }))
            }
            PreviewAction::CreateTarget { url } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                let tid = cdp
                    .create_target(url.as_deref().unwrap_or("about:blank"))
                    .await?;
                Ok(json!({ "targetId": tid }))
            }
            PreviewAction::Navigate { url } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                cdp.navigate(&url).await
            }
            PreviewAction::Evaluate { expression } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                cdp.evaluate(&expression).await
            }
            PreviewAction::Screenshot { format } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                cdp.capture_screenshot(format.as_deref()).await
            }
            PreviewAction::MouseEvent { params } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                cdp.dispatch_mouse_event(&params).await
            }
            PreviewAction::KeyEvent { params } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                cdp.dispatch_key_event(&params).await
            }
            PreviewAction::SetViewport {
                width,
                height,
                device_scale_factor,
                mobile,
            } => {
                let mut guard = self.inner.lock().await;
                let cdp = guard.cdp.as_mut().ok_or_else(|| {
                    ServerRuntimeError::Conflict("no active CDP session".to_owned())
                })?;
                cdp.set_device_metrics(width, height, device_scale_factor, mobile)
                    .await
            }
            PreviewAction::Close => {
                self.close().await;
                Ok(json!({ "closed": true }))
            }
        }
    }

    /// Close active CDP socket and kill any spawned browser process tree.
    pub async fn close(&self) {
        let (mut cdp, child) = {
            let mut guard = self.inner.lock().await;
            guard.ws_url = None;
            (guard.cdp.take(), guard.child.take())
        };
        if let Some(ref mut conn) = cdp {
            conn.close().await;
        }
        drop(child);
    }
}

fn spawn_chromium_and_wait_ws(
    executable: &Path,
) -> Result<(BrowserChild, String), ServerRuntimeError> {
    let mut args = vec![
        "--headless=new".to_owned(),
        "--remote-debugging-port=0".to_owned(),
        "--disable-gpu".to_owned(),
    ];
    if std::env::var("T3CODE_SERVER_BROWSER_SANDBOX").as_deref() == Ok("0") {
        args.push("--no-sandbox".to_owned());
    }
    let env = SanitizedChildEnv::from_os_env();
    let spec = SpawnSpec::new(executable.as_os_str())
        .args(&args)
        .env(env)
        .stdio(PipeSetConfig::piped())
        .windows_no_window(true);

    let mut supervisor =
        JobProcessSupervisor::new().map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    supervisor.set_kill_on_drop(true);
    let (token, mut pipes) = supervisor
        .spawn(&spec)
        .map_err(|e| ServerRuntimeError::Process(e.to_string()))?;
    let _ = pipes.take_stdin();
    let stderr = pipes
        .take_stderr()
        .ok_or_else(|| ServerRuntimeError::Process("browser stderr pipe missing".to_owned()))?;

    let mut reader = BufReader::new(stderr);
    let mut captured = String::new();
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        captured.push_str(&line);
        if let Some(pos) = line.find("DevTools listening on ") {
            let ws_url = line[pos + "DevTools listening on ".len()..]
                .trim()
                .to_owned();
            return Ok((BrowserChild { supervisor, token }, ws_url));
        }
        line.clear();
    }
    let exe_str = executable.to_string_lossy();
    let is_linux = cfg!(target_os = "linux");
    if let Some(diag) =
        diagnose_browser_launch_failure(&exe_str, &captured, "t3 browser setup", false, is_linux)
    {
        return Err(ServerRuntimeError::Process(format!("{diag:?}")));
    }
    Err(ServerRuntimeError::Process(format!(
        "browser exited without CDP endpoint: {}",
        captured.trim()
    )))
}

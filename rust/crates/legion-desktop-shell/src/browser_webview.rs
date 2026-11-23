//! Desktop browser webview host behaviors: context menu `Reload` (`#22650`), `Cmd+T`/`Ctrl+T`
//! forwarding (`#39769`), destroyed guest handle cleanup (`#40177`), and workstation-locked hidden
//! frame capture leases (`#39198`).

use std::collections::{BTreeMap, BTreeSet};

pub use crate::browser_webview_controls::{
    BrowserContextMenuItem, ContextMenuEditFlags, FocusedWebviewShortcutAction,
    NativeContextMenuItem, build_browser_context_menu, build_native_context_menu,
    escape_native_menu_label, match_focused_browser_webview_shortcut,
};

/// Mockable web contents handle representing an Electron/WRY guest or main window webview.
#[derive(Debug, Clone)]
pub struct WebContentsState {
    pub id: u64,
    pub destroyed: bool,
    pub reload_count: u64,
    pub background_throttling_calls: Vec<bool>,
    pub background_throttling: bool,
    pub hidden: bool,
}

impl WebContentsState {
    /// Construct a fresh, live [`WebContentsState`].
    pub const fn new(id: u64) -> Self {
        Self {
            id,
            destroyed: false,
            reload_count: 0,
            background_throttling_calls: Vec::new(),
            background_throttling: true,
            hidden: false,
        }
    }

    /// Sets `backgroundThrottling` (`#39198`), recording each toggle for verification.
    pub fn set_background_throttling(&mut self, enabled: bool) {
        self.background_throttling = enabled;
        self.background_throttling_calls.push(enabled);
    }

    /// Triggers reload ONLY when `!self.destroyed` (`#22650`).
    pub fn reload_if_alive(&mut self) -> bool {
        if self.destroyed {
            return false;
        }
        self.reload_count += 1;
        true
    }
}

/// Preview tab tracked by [`DesktopPreviewWebviewHost`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTabRecord {
    pub tab_id: String,
    pub web_contents_id: Option<u64>,
    pub favicon: Option<String>,
    pub is_browser_contents: bool,
}

/// Error returned when a preview tab no longer holds a live webview (`#40177`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewWebviewError {
    TabNotFound(String),
    WebviewNotInitialized(String),
}

/// Coordinates hosted browser webviews, clean detachment when destroyed (`#40177`), context menu
/// `Reload` dispatch (`#22650`), and unthrottled hidden frame capture while Windows is locked (`#39198`).
#[derive(Debug, Default)]
pub struct DesktopPreviewWebviewHost {
    tabs: BTreeMap<String, PreviewTabRecord>,
    web_contents: BTreeMap<u64, WebContentsState>,
    attached: BTreeMap<u64, String>,
    capture_leases: BTreeSet<(String, String)>,
    main_window_wc: Option<WebContentsState>,
    workstation_locked: bool,
}

impl DesktopPreviewWebviewHost {
    /// Construct an empty [`DesktopPreviewWebviewHost`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs or replaces the host main-window webContents tracked for throttling leases (`#39198`).
    pub fn set_main_window(&mut self, wc_id: u64) {
        self.main_window_wc = Some(WebContentsState::new(wc_id));
    }

    /// Returns the main window's `WebContentsState` if set.
    pub const fn main_window(&self) -> Option<&WebContentsState> {
        self.main_window_wc.as_ref()
    }

    /// Registers an attached guest webContents before assigning it to a tab.
    pub fn prepare_webview(&mut self, wc_id: u64, attachment_tag: impl Into<String>) {
        self.web_contents
            .insert(wc_id, WebContentsState::new(wc_id));
        self.attached.insert(wc_id, attachment_tag.into());
    }

    /// Creates a preview tab entry without a bound webContents yet.
    pub fn create_tab(&mut self, tab_id: impl Into<String>) {
        let id = tab_id.into();
        self.tabs.insert(
            id.clone(),
            PreviewTabRecord {
                tab_id: id,
                web_contents_id: None,
                favicon: None,
                is_browser_contents: true,
            },
        );
    }

    /// Binds a live `wc_id` to `tab_id`.
    pub fn register_webview(
        &mut self,
        tab_id: &str,
        wc_id: u64,
    ) -> Result<(), PreviewWebviewError> {
        let tab = self
            .tabs
            .get_mut(tab_id)
            .ok_or_else(|| PreviewWebviewError::TabNotFound(tab_id.to_owned()))?;
        if self.web_contents.get(&wc_id).is_none_or(|wc| wc.destroyed) {
            return Err(PreviewWebviewError::WebviewNotInitialized(
                tab_id.to_owned(),
            ));
        }
        tab.web_contents_id = Some(wc_id);
        Ok(())
    }

    /// Marks `wc_id` destroyed and cleanly drops/detaches its tab binding (`#40177`) while
    /// releasing any capture lease held by that tab.
    pub fn on_web_contents_destroyed(&mut self, wc_id: u64) {
        if let Some(wc) = self.web_contents.get_mut(&wc_id) {
            wc.destroyed = true;
        }
        let matching_tabs: Vec<String> = self
            .tabs
            .values()
            .filter(|t| t.web_contents_id == Some(wc_id))
            .map(|t| t.tab_id.clone())
            .collect();
        for tab_id in matching_tabs {
            self.detach_destroyed_web_contents(&tab_id, wc_id);
        }
        self.attached.remove(&wc_id);
        self.web_contents.remove(&wc_id);
    }

    /// Clears a tab's dead guest handle so callers observe `WebviewNotInitialized` until remounted (`#40177`).
    pub fn detach_destroyed_web_contents(&mut self, tab_id: &str, wc_id: u64) {
        let is_alive = self
            .web_contents
            .get(&wc_id)
            .is_some_and(|wc| !wc.destroyed);
        if is_alive {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(tab_id)
            && tab.web_contents_id == Some(wc_id)
        {
            tab.web_contents_id = None;
            tab.favicon = None;
        }
        let stale_leases: Vec<(String, String)> = self
            .capture_leases
            .iter()
            .filter(|(_, tid)| tid == tab_id)
            .cloned()
            .collect();
        for lease in stale_leases {
            let _ = self.set_server_capture_active(&lease.0, &lease.1, false);
        }
    }

    /// Opens picture-in-picture only when `tab_id` owns a live non-destroyed webContents (`#40177`).
    pub fn open_picture_in_picture(&self, tab_id: &str) -> Result<u64, PreviewWebviewError> {
        let tab = self
            .tabs
            .get(tab_id)
            .ok_or_else(|| PreviewWebviewError::TabNotFound(tab_id.to_owned()))?;
        let Some(wc_id) = tab.web_contents_id else {
            return Err(PreviewWebviewError::WebviewNotInitialized(
                tab_id.to_owned(),
            ));
        };
        let Some(wc) = self.web_contents.get(&wc_id) else {
            return Err(PreviewWebviewError::WebviewNotInitialized(
                tab_id.to_owned(),
            ));
        };
        if wc.destroyed {
            return Err(PreviewWebviewError::WebviewNotInitialized(
                tab_id.to_owned(),
            ));
        }
        Ok(wc_id)
    }

    /// Reloads ONLY the specified webContents ID if alive (`#22650`).
    pub fn reload_web_contents(&mut self, wc_id: u64) -> bool {
        self.web_contents
            .get_mut(&wc_id)
            .is_some_and(WebContentsState::reload_if_alive)
    }

    /// Tracks workstation lock state (Windows lock hides/covers host window, requiring capture leases
    /// to keep hidden browser tabs painting, `#39198`).
    pub fn set_workstation_locked(&mut self, locked: bool) {
        self.workstation_locked = locked;
        if let Some(ref mut main_wc) = self.main_window_wc {
            main_wc.hidden = locked;
        }
    }

    /// Starts or stops server-driven frame capture for `(thread_id, tab_id)`, unthrottling both
    /// the guest webview and main window even while hidden/locked (`#39198`).
    pub fn set_server_capture_active(
        &mut self,
        thread_id: &str,
        tab_id: &str,
        active: bool,
    ) -> Result<(), PreviewWebviewError> {
        let key = (thread_id.to_owned(), tab_id.to_owned());
        if active {
            if !self.capture_leases.contains(&key) {
                let tab = self
                    .tabs
                    .get(tab_id)
                    .ok_or_else(|| PreviewWebviewError::TabNotFound(tab_id.to_owned()))?;
                let wc_id = tab
                    .web_contents_id
                    .ok_or_else(|| PreviewWebviewError::WebviewNotInitialized(tab_id.to_owned()))?;
                let guest = self
                    .web_contents
                    .get_mut(&wc_id)
                    .filter(|w| !w.destroyed)
                    .ok_or_else(|| PreviewWebviewError::WebviewNotInitialized(tab_id.to_owned()))?;
                guest.set_background_throttling(false);
                if self.capture_leases.is_empty()
                    && let Some(ref mut main_wc) = self.main_window_wc
                    && !main_wc.destroyed
                {
                    main_wc.set_background_throttling(false);
                }
                self.capture_leases.insert(key);
            }
        } else if self.capture_leases.remove(&key) {
            if let Some(tab) = self.tabs.get(tab_id)
                && let Some(wc_id) = tab.web_contents_id
                && !self.capture_leases.iter().any(|(_, tid)| tid == tab_id)
                && let Some(guest) = self.web_contents.get_mut(&wc_id)
                && !guest.destroyed
            {
                guest.set_background_throttling(true);
            }
            if self.capture_leases.is_empty()
                && let Some(ref mut main_wc) = self.main_window_wc
                && !main_wc.destroyed
            {
                main_wc.set_background_throttling(true);
            }
        }
        Ok(())
    }

    /// Read tab metadata (`web_contents_id`, `favicon`, etc.).
    pub fn tab(&self, tab_id: &str) -> Option<&PreviewTabRecord> {
        self.tabs.get(tab_id)
    }

    /// Read guest webContents state.
    pub fn web_contents_state(&self, wc_id: u64) -> Option<&WebContentsState> {
        self.web_contents.get(&wc_id)
    }
}

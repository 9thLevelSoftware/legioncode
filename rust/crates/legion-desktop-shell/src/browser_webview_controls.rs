//! Context menu `Reload` (`#22650`) and focused browser webview `Cmd+T`/`Ctrl+T` forwarding (`#39769`).

use legion_surfaces::desktop_shortcuts::{
    DesktopKeyChord, DesktopPlatform, shortcut_system_conflict,
};
use serde::{Deserialize, Serialize};

/// Context-menu edit flags supplied by Electron/WebView2 guest or host pages (`#22650`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "Wire schema mirrors Electron ContextMenuParams.editFlags booleans directly"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMenuEditFlags {
    pub can_cut: bool,
    pub can_copy: bool,
    pub can_paste: bool,
    pub can_select_all: bool,
}

/// Logical context menu item constructed for main vs embedded browser webview pages (`#22650`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserContextMenuItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accelerator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_web_contents_id: Option<u64>,
}

/// Builds the context menu template for a page, prepending `Reload` (without accelerator) and a
/// separator ONLY when `is_browser_contents` is `true` (guest or popup browser webviews) (`#22650`).
pub fn build_browser_context_menu(
    is_browser_contents: bool,
    web_contents_id: u64,
    edit_flags: ContextMenuEditFlags,
) -> Vec<BrowserContextMenuItem> {
    let mut items = Vec::new();
    if is_browser_contents {
        items.push(BrowserContextMenuItem {
            role: None,
            label: Some("Reload".to_owned()),
            item_type: None,
            enabled: true,
            accelerator: None,
            target_web_contents_id: Some(web_contents_id),
        });
        items.push(BrowserContextMenuItem {
            role: None,
            label: None,
            item_type: Some("separator".to_owned()),
            enabled: true,
            accelerator: None,
            target_web_contents_id: None,
        });
    }
    for (role, enabled) in [
        ("cut", edit_flags.can_cut),
        ("copy", edit_flags.can_copy),
        ("paste", edit_flags.can_paste),
        ("selectAll", edit_flags.can_select_all),
    ] {
        items.push(BrowserContextMenuItem {
            role: Some(role.to_owned()),
            label: None,
            item_type: None,
            enabled,
            accelerator: None,
            target_web_contents_id: None,
        });
    }
    items
}

/// Host command emitted when a focused browser webview intercepts a desktop UI shortcut (`#39769`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FocusedWebviewShortcutAction {
    /// Forward `Cmd+T` / `Ctrl+T` to open the `RightPanel` new-tab menu (`rightPanel.new`).
    RightPanelNewTab,
}

/// Evaluates a keyboard chord received while a browser guest webview has focus (`#39769`).
///
/// Returns `Some(`[`FocusedWebviewShortcutAction::RightPanelNewTab`]`)` when `Cmd+T` (macOS) or
/// `Ctrl+T` (Windows/Linux) is pressed with no extra modifiers so the desktop forwards
/// `"rightPanel.new"` instead of letting the guest swallow it.
pub fn match_focused_browser_webview_shortcut(
    platform: DesktopPlatform,
    chord: &DesktopKeyChord,
) -> Option<FocusedWebviewShortcutAction> {
    if chord.alt_key || chord.shift_key || !chord.key.eq_ignore_ascii_case("t") {
        return None;
    }
    let has_primary = match platform {
        DesktopPlatform::MacOs => chord.meta_key && !chord.ctrl_key,
        DesktopPlatform::Windows | DesktopPlatform::Linux => chord.ctrl_key && !chord.meta_key,
    };
    if !has_primary {
        return None;
    }
    let mod_chord = DesktopKeyChord::new("t").with_mod();
    if shortcut_system_conflict(&mod_chord).is_some_and(|msg| msg.contains("New Tab")) {
        Some(FocusedWebviewShortcutAction::RightPanelNewTab)
    } else {
        None
    }
}

/// Escapes single `&` characters as `&&` so native Windows/Linux menus display literal
/// ampersands instead of parsing them as mnemonic accelerators (`#41097`).
#[must_use]
pub fn escape_native_menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

/// Logical native context menu item mirrored from `DesktopContextMenuItem` (`#41097`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeContextMenuItem {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NativeContextMenuItem>,
}

/// Escapes ampersands across every label and nested submenu item before handing the menu template
/// to the OS menu toolkit (`#41097`).
#[must_use]
pub fn build_native_context_menu(items: &[NativeContextMenuItem]) -> Vec<NativeContextMenuItem> {
    items
        .iter()
        .map(|item| NativeContextMenuItem {
            id: item.id.clone(),
            label: escape_native_menu_label(&item.label),
            disabled: item.disabled,
            checked: item.checked,
            children: build_native_context_menu(&item.children),
        })
        .collect()
}

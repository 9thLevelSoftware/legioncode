//! Desktop shortcut formatting, conflict warnings, registration errors, and
//! platform menu zoom accelerators (`apps/desktop/src/snapShot/snapShot.ts`,
//! `apps/desktop/src/window/DesktopApplicationMenu.ts`, `packages/contracts/src/settings.ts`).

use serde::{Deserialize, Serialize};

/// Modifier key supported by double-tap modifier-pair shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopModifier {
    /// Either Shift key.
    Shift,
    /// Super / Command / Windows key.
    Meta,
    /// Control key.
    Control,
    /// Alt / Option key.
    Alt,
}

impl DesktopModifier {
    /// Human-readable double-tap label (`snapShotModifierPairLabel`).
    #[must_use]
    pub fn pair_label(self, is_apple: bool) -> String {
        let label = match (self, is_apple) {
            (Self::Shift, _) => "Shift",
            (Self::Meta, true) => "Command",
            (Self::Meta, false) => "Super",
            (Self::Control, true) => "Control",
            (Self::Control, false) => "Ctrl",
            (Self::Alt, true) => "Option",
            (Self::Alt, false) => "Alt",
        };
        format!("{label} + {label}")
    }
}

/// A modifier + key chord (`SnapShotKeyChord` / `KeybindingShortcut`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors KeybindingShortcut five-modifier schema in packages/contracts"
)]
pub struct DesktopKeyChord {
    /// Primary key token (typically lowercase from `parseKeybindingShortcut`).
    pub key: String,
    /// Platform command/control modifier (`CommandOrControl`).
    pub mod_key: bool,
    /// Super / Meta / Windows key.
    pub meta_key: bool,
    /// Explicit Control key.
    pub ctrl_key: bool,
    /// Alt / Option key.
    pub alt_key: bool,
    /// Shift key.
    pub shift_key: bool,
}

impl DesktopKeyChord {
    /// Builds a chord with all modifier flags cleared.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            mod_key: false,
            meta_key: false,
            ctrl_key: false,
            alt_key: false,
            shift_key: false,
        }
    }

    /// Enables the `CommandOrControl` (`mod`) modifier.
    #[must_use]
    pub const fn with_mod(mut self) -> Self {
        self.mod_key = true;
        self
    }

    /// Enables the `Super` (`meta`) modifier.
    #[must_use]
    pub const fn with_meta(mut self) -> Self {
        self.meta_key = true;
        self
    }

    /// Enables the explicit `Control` modifier.
    #[must_use]
    pub const fn with_ctrl(mut self) -> Self {
        self.ctrl_key = true;
        self
    }

    /// Enables the `Alt` modifier.
    #[must_use]
    pub const fn with_alt(mut self) -> Self {
        self.alt_key = true;
        self
    }

    /// Enables the `Shift` modifier.
    #[must_use]
    pub const fn with_shift(mut self) -> Self {
        self.shift_key = true;
        self
    }

    /// Number of active modifiers in this chord.
    #[must_use]
    pub const fn modifier_count(&self) -> usize {
        self.mod_key as usize
            + self.meta_key as usize
            + self.ctrl_key as usize
            + self.alt_key as usize
            + self.shift_key as usize
    }
}

/// Configured desktop shortcut: either a double-tap modifier pair or a key chord.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DesktopShortcut {
    /// Legacy double-tap Shift shortcut.
    BothShiftKeys,
    /// Double-tap modifier shortcut.
    ModifierPair {
        /// Modifier key tapped twice.
        modifier: DesktopModifier,
    },
    /// Standard Electron key chord.
    Chord(DesktopKeyChord),
}

impl DesktopShortcut {
    /// Returns the modifier if this shortcut is a modifier-pair shortcut.
    #[must_use]
    pub const fn modifier_pair(&self) -> Option<DesktopModifier> {
        match self {
            Self::BothShiftKeys => Some(DesktopModifier::Shift),
            Self::ModifierPair { modifier } => Some(*modifier),
            Self::Chord(_) => None,
        }
    }
}

/// Formats a key chord as an Electron accelerator (`toElectronAccelerator`).
#[must_use]
pub fn to_electron_accelerator(shortcut: &DesktopKeyChord) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(6);
    if shortcut.mod_key {
        parts.push("CommandOrControl".to_owned());
    }
    if shortcut.meta_key {
        parts.push("Super".to_owned());
    }
    if shortcut.ctrl_key {
        parts.push("Control".to_owned());
    }
    if shortcut.alt_key {
        parts.push("Alt".to_owned());
    }
    if shortcut.shift_key {
        parts.push("Shift".to_owned());
    }
    let mapped = match shortcut.key.to_ascii_lowercase().as_str() {
        " " => "Space".to_owned(),
        "+" => "Plus".to_owned(),
        "arrowdown" => "Down".to_owned(),
        "arrowleft" => "Left".to_owned(),
        "arrowright" => "Right".to_owned(),
        "arrowup" => "Up".to_owned(),
        "escape" => "Esc".to_owned(),
        _ => shortcut.key.to_uppercase(),
    };
    parts.push(mapped);
    parts.join("+")
}

/// Returns whether two desktop shortcuts denote the same shortcut (`sameSnapShotShortcut`).
#[must_use]
pub fn same_shortcut(left: &DesktopShortcut, right: &DesktopShortcut) -> bool {
    match (left.modifier_pair(), right.modifier_pair()) {
        (Some(l), Some(r)) => l == r,
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => match (left, right) {
            (DesktopShortcut::Chord(l), DesktopShortcut::Chord(r)) => {
                to_electron_accelerator(l) == to_electron_accelerator(r)
            }
            _ => false,
        },
    }
}

/// Warns when a chord collides with common OS, app, or terminal bindings.
#[must_use]
pub fn shortcut_system_conflict(shortcut: &DesktopKeyChord) -> Option<String> {
    if shortcut.modifier_count() != 1 {
        return None;
    }
    if shortcut.shift_key {
        return Some(
            "Shift combinations are used for typing and text selection. Add another modifier."
                .to_owned(),
        );
    }
    let key = shortcut.key.to_ascii_lowercase();
    if shortcut.mod_key {
        let action = common_mod_action(key.as_str())?;
        return Some(format!("This shortcut is {action} in most apps."));
    }
    if shortcut.ctrl_key && matches!(key.as_str(), "c" | "d" | "z") {
        return Some("This shortcut controls running commands in terminals.".to_owned());
    }
    if shortcut.alt_key && key == "tab" {
        return Some("The system uses Alt+Tab to switch apps.".to_owned());
    }
    if shortcut.meta_key && matches!(key.as_str(), "l" | " ") {
        return Some("The system already uses this shortcut.".to_owned());
    }
    None
}

const fn common_mod_action(key: &str) -> Option<&'static str> {
    match key.as_bytes() {
        b"a" => Some("Select All"),
        b"c" => Some("Copy"),
        b"f" => Some("Find"),
        b"n" => Some("New"),
        b"o" => Some("Open"),
        b"p" => Some("Print"),
        b"q" => Some("Quit"),
        b"s" => Some("Save"),
        b"t" => Some("New Tab"),
        b"v" => Some("Paste"),
        b"w" => Some("Close Window"),
        b"x" => Some("Cut"),
        b"z" => Some("Undo"),
        _ => None,
    }
}

/// Builds the user-facing message when global shortcut registration fails.
#[must_use]
pub fn shortcut_registration_failure_message(shortcut: &DesktopShortcut, is_apple: bool) -> String {
    match shortcut.modifier_pair() {
        Some(modifier) => {
            format!(
                "{} is not available on this system.",
                modifier.pair_label(is_apple)
            )
        }
        None => "This shortcut is already used by the system or another app.".to_owned(),
    }
}

/// Host desktop platform used to build platform-specific menu accelerators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopPlatform {
    /// macOS (`darwin`).
    MacOs,
    /// Windows (`win32`).
    Windows,
    /// Linux.
    Linux,
}

/// Zoom action triggered from the desktop `View` menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopZoomAction {
    /// Reset main window zoom to actual size (`0`).
    Reset,
    /// Zoom main window in.
    In,
    /// Zoom main window out.
    Out,
}

/// Accelerator entry registered for desktop zoom menu actions (`DesktopApplicationMenu.ts:335-372`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopMenuAccelerator {
    /// Zoom action invoked when triggered.
    pub action: DesktopZoomAction,
    /// Electron accelerator string.
    pub accelerator: &'static str,
    /// Whether the menu item is visible in the View menu.
    pub visible: bool,
}

/// Returns the View menu zoom accelerators for `platform`, including macOS `CmdOrCtrl+Shift+=`.
#[must_use]
pub fn zoom_accelerators_for_platform(platform: DesktopPlatform) -> Vec<DesktopMenuAccelerator> {
    let mut entries = vec![
        DesktopMenuAccelerator {
            action: DesktopZoomAction::Reset,
            accelerator: "CmdOrCtrl+0",
            visible: true,
        },
        DesktopMenuAccelerator {
            action: DesktopZoomAction::In,
            accelerator: "CmdOrCtrl+=",
            visible: true,
        },
        DesktopMenuAccelerator {
            action: DesktopZoomAction::In,
            accelerator: "CmdOrCtrl+Plus",
            visible: false,
        },
    ];
    if matches!(platform, DesktopPlatform::MacOs) {
        entries.push(DesktopMenuAccelerator {
            action: DesktopZoomAction::In,
            accelerator: "CmdOrCtrl+Shift+=",
            visible: false,
        });
    }
    entries.extend([
        DesktopMenuAccelerator {
            action: DesktopZoomAction::Out,
            accelerator: "CmdOrCtrl+-",
            visible: true,
        },
        DesktopMenuAccelerator {
            action: DesktopZoomAction::Reset,
            accelerator: "CmdOrCtrl+num0",
            visible: false,
        },
        DesktopMenuAccelerator {
            action: DesktopZoomAction::In,
            accelerator: "CmdOrCtrl+numadd",
            visible: false,
        },
        DesktopMenuAccelerator {
            action: DesktopZoomAction::Out,
            accelerator: "CmdOrCtrl+numsub",
            visible: false,
        },
    ]);
    entries
}

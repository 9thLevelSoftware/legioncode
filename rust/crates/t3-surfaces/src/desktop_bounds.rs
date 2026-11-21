//! Display-clamped `WindowBounds` sanitizer for the desktop shell.

use serde::{Deserialize, Serialize};

/// Minimum main window width in DIPs (`DesktopAppSettings.ts:55`).
pub const MIN_MAIN_WINDOW_WIDTH: i32 = 400;
/// Minimum main window height in DIPs (`DesktopAppSettings.ts:55`).
pub const MIN_MAIN_WINDOW_HEIGHT: i32 = 360;
/// Default main window width in DIPs (`DesktopAppSettings.ts:66`).
pub const DEFAULT_MAIN_WINDOW_WIDTH: i32 = 1100;
/// Default main window height in DIPs (`DesktopAppSettings.ts:66`).
pub const DEFAULT_MAIN_WINDOW_HEIGHT: i32 = 780;
/// Edge tolerance when restoring snapped bounds (`DesktopWindow.ts:173`).
pub const SAVED_BOUNDS_EDGE_TOLERANCE_PX: i32 = 1;

/// Physical/DIP rectangle representing a connected display (`DesktopWindow.ts:170`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayRect {
    /// Left coordinate in DIPs.
    pub x: i32,
    /// Top coordinate in DIPs.
    pub y: i32,
    /// Width in DIPs.
    pub width: i32,
    /// Height in DIPs.
    pub height: i32,
}

/// Persisted or active main window bounds (`DesktopAppSettings.ts:59-65`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowBounds {
    /// Left coordinate in DIPs.
    pub x: i32,
    /// Top coordinate in DIPs.
    pub y: i32,
    /// Width in DIPs (minimum `400` when valid).
    pub width: i32,
    /// Height in DIPs (minimum `360` when valid).
    pub height: i32,
}

/// Initial window geometry chosen at startup (`DesktopWindow.ts:252-263`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InitialWindowBounds {
    /// Persisted bounds fit cleanly on a connected display.
    Restored(WindowBounds),
    /// Persisted bounds were missing, invalid, or off-screen -> use default `1100x780`.
    DefaultSize {
        /// `1100`.
        width: i32,
        /// `780`.
        height: i32,
    },
}

impl WindowBounds {
    /// Validates minimum main-window dimensions (`width >= 400`, `height >= 360`).
    pub const fn meets_minimum_size(self) -> bool {
        self.width >= MIN_MAIN_WINDOW_WIDTH && self.height >= MIN_MAIN_WINDOW_HEIGHT
    }

    /// Checks whether `self` fits within `display` with `SAVED_BOUNDS_EDGE_TOLERANCE_PX` (`1px`).
    pub fn fits_within_display(self, display: DisplayRect) -> bool {
        if display.width <= 0 || display.height <= 0 || self.width <= 0 || self.height <= 0 {
            return false;
        }
        let tol = i64::from(SAVED_BOUNDS_EDGE_TOLERANCE_PX);
        let x = i64::from(self.x);
        let y = i64::from(self.y);
        let right = x + i64::from(self.width);
        let bottom = y + i64::from(self.height);
        let dx = i64::from(display.x);
        let dy = i64::from(display.y);
        let dright = dx + i64::from(display.width);
        let dbottom = dy + i64::from(display.height);
        x >= dx - tol && y >= dy - tol && right <= dright + tol && bottom <= dbottom + tol
    }

    /// Matches `resolveInitialMainWindowBounds` (`DesktopWindow.ts:252-263`).
    pub fn resolve_initial(
        persisted: Option<Self>,
        displays: &[DisplayRect],
    ) -> InitialWindowBounds {
        if let Some(bounds) = persisted
            && bounds.meets_minimum_size()
            && displays.iter().any(|d| bounds.fits_within_display(*d))
        {
            return InitialWindowBounds::Restored(bounds);
        }
        InitialWindowBounds::DefaultSize {
            width: DEFAULT_MAIN_WINDOW_WIDTH,
            height: DEFAULT_MAIN_WINDOW_HEIGHT,
        }
    }

    /// Sanitizes and clamps `self` onto the best-overlapping connected display (or primary
    /// display if off-screen), enforcing minimum dimensions (`400x360` when display allows) and
    /// ensuring the window remains completely inside the target display work area.
    #[must_use]
    pub fn sanitize_clamped(self, displays: &[DisplayRect]) -> Self {
        let valid_displays: Vec<DisplayRect> = displays
            .iter()
            .copied()
            .filter(|d| d.width > 0 && d.height > 0)
            .collect();
        let Some(target) = best_matching_display(self, &valid_displays)
            .or_else(|| valid_displays.first().copied())
        else {
            return Self {
                x: self.x,
                y: self.y,
                width: self.width.max(MIN_MAIN_WINDOW_WIDTH),
                height: self.height.max(MIN_MAIN_WINDOW_HEIGHT),
            };
        };

        let min_w = MIN_MAIN_WINDOW_WIDTH.min(target.width);
        let min_h = MIN_MAIN_WINDOW_HEIGHT.min(target.height);
        let width = self.width.clamp(min_w, target.width);
        let height = self.height.clamp(min_h, target.height);
        let max_x = target.x.saturating_add(target.width - width);
        let max_y = target.y.saturating_add(target.height - height);
        Self {
            x: self.x.clamp(target.x, max_x),
            y: self.y.clamp(target.y, max_y),
            width,
            height,
        }
    }

    /// Ports `normalBoundsOnWindowDisplay` (`DesktopWindow.ts:211-230`).
    #[must_use]
    pub fn normal_on_window_display(self, window: Self, displays: &[DisplayRect]) -> Self {
        let Some(target) = best_matching_display(window, displays) else {
            return self;
        };
        if self.fits_within_display(target) {
            return self;
        }
        let source = best_matching_display(self, displays).unwrap_or(target);
        let width = self.width.min(target.width).max(1);
        let height = self.height.min(target.height).max(1);
        let max_x = target.x.saturating_add(target.width - width);
        let max_y = target.y.saturating_add(target.height - height);
        let shifted_x = i64::from(target.x) + i64::from(self.x) - i64::from(source.x);
        let shifted_y = i64::from(target.y) + i64::from(self.y) - i64::from(source.y);
        let x = i32::try_from(shifted_x.clamp(i64::from(target.x), i64::from(max_x)))
            .unwrap_or(target.x);
        let y = i32::try_from(shifted_y.clamp(i64::from(target.y), i64::from(max_y)))
            .unwrap_or(target.y);
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

fn overlap_area(bounds: WindowBounds, display: DisplayRect) -> i64 {
    if bounds.width <= 0 || bounds.height <= 0 || display.width <= 0 || display.height <= 0 {
        return 0;
    }
    let left = i64::from(bounds.x).max(i64::from(display.x));
    let right = (i64::from(bounds.x) + i64::from(bounds.width))
        .min(i64::from(display.x) + i64::from(display.width));
    let top = i64::from(bounds.y).max(i64::from(display.y));
    let bottom = (i64::from(bounds.y) + i64::from(bounds.height))
        .min(i64::from(display.y) + i64::from(display.height));
    let w = right - left;
    let h = bottom - top;
    if w > 0 && h > 0 {
        w.saturating_mul(h)
    } else {
        0
    }
}

fn best_matching_display(bounds: WindowBounds, displays: &[DisplayRect]) -> Option<DisplayRect> {
    let mut best: Option<DisplayRect> = None;
    let mut best_area = 0_i64;
    for &display in displays {
        let area = overlap_area(bounds, display);
        if area > best_area {
            best_area = area;
            best = Some(display);
        }
    }
    best
}

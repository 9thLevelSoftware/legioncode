//! Pure-Rust host-testable bridge helpers for [`legion_terminal_vt`].

use legion_terminal_vt::abi::GhosttyCursorShape;
use legion_terminal_vt::palette::{Rgba, TerminalThemeConfig, parse_color, parse_palette_entry};
use legion_terminal_vt::snapshot::try_encode_lcvt;
use legion_terminal_vt::{TerminalGrid, TerminalSnapshot, decode_lcvt, sanitize_bracketed_paste};
use serde::{Deserialize, Serialize};

/// Sanitizes untrusted clipboard text for safe bracketed-paste insertion.
pub fn sanitize_bracketed_paste_str(raw: &str) -> String {
    sanitize_bracketed_paste(raw)
}

/// Renders `ansi_input` on a `cols x rows` VT grid and encodes a `"LCVT"` v2 snapshot.
pub fn encode_lcvt_from_ansi(cols: u16, rows: u16, ansi_input: &str) -> Result<Vec<u8>, String> {
    let mut grid = TerminalGrid::try_new(cols, rows).map_err(|err| err.to_string())?;
    grid.feed_str(ansi_input);
    let snapshot = grid.snapshot();
    try_encode_lcvt(&snapshot).map_err(|err| err.to_string())
}

/// Decodes `"LCVT"` v1/v2 binary snapshot bytes and returns a JSON summary.
pub fn decode_lcvt_summary_json(bytes: &[u8]) -> Result<String, String> {
    let snapshot = decode_lcvt(bytes).map_err(|err| err.to_string())?;
    serialize_snapshot_summary(&snapshot, None, None, None)
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PaletteEntryInput {
    Object { index: u8, color: String },
    Tuple(u8, String),
    Line(String),
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ApplyThemeRequest {
    #[serde(alias = "config", alias = "ghosttyConfig", alias = "config_text")]
    config_text: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    #[serde(alias = "cursor_color")]
    cursor_color: Option<String>,
    #[serde(alias = "overrides")]
    palette: Option<Vec<PaletteEntryInput>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaletteOverrideOut {
    index: u8,
    hex: String,
    argb: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyThemeResponse {
    override_count: usize,
    overrides: Vec<PaletteOverrideOut>,
    foreground: String,
    background: String,
    cursor_color: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotSummaryResponse {
    cols: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    cursor_visible: bool,
    cursor_style: &'static str,
    cursor_blinking: bool,
    alternate_screen: bool,
    title: Option<String>,
    bell_count: Option<u64>,
    scrollback_len: Option<usize>,
    foreground: String,
    background: String,
    cursor_color: String,
    palette_override_count: usize,
    palette_overrides: Vec<PaletteOverrideOut>,
    cell_count: usize,
    non_empty_cell_count: usize,
    lines: Vec<String>,
    viewport_text: String,
}

/// Host-testable VT terminal state machine backing `WasmTerminalVt`.
#[derive(Debug, Clone)]
pub struct TerminalVtBridge {
    grid: TerminalGrid,
}

impl TerminalVtBridge {
    /// Creates a bounded VT terminal grid validated to `1..=1024 x 1..=512`.
    pub fn new(cols: u16, rows: u16) -> Result<Self, String> {
        let grid = TerminalGrid::try_new(cols, rows).map_err(|err| err.to_string())?;
        Ok(Self { grid })
    }

    /// Feeds UTF-8 / ANSI / OSC text incrementally into the VT parser.
    pub fn feed(&mut self, text: &str) {
        self.grid.feed_str(text);
    }

    /// Feeds raw PTY bytes incrementally into the VT parser.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        self.grid.feed(bytes);
    }

    /// Applies Ghostty theme configuration / sparse ANSI palette overrides (`BUG-25`).
    pub fn apply_theme_json(&mut self, json: &str) -> Result<String, String> {
        let req: ApplyThemeRequest =
            serde_json::from_str(json).map_err(|err| format!("invalid theme JSON: {err}"))?;

        let mut theme = match req.config_text.as_deref() {
            Some(cfg) => TerminalThemeConfig::parse(cfg),
            None => TerminalThemeConfig::new(),
        };
        if let Some(fg) = req.foreground.as_deref() {
            theme.foreground = Some(parse_color(fg).map_err(|err| err.to_string())?);
        }
        if let Some(bg) = req.background.as_deref() {
            theme.background = Some(parse_color(bg).map_err(|err| err.to_string())?);
        }
        if let Some(cc) = req.cursor_color.as_deref() {
            theme.cursor_color = Some(parse_color(cc).map_err(|err| err.to_string())?);
        }
        if let Some(entries) = req.palette {
            let mut pairs = Vec::with_capacity(entries.len());
            for entry in entries {
                pairs.push(parse_palette_input(entry)?);
            }
            theme.palette.apply_theme_overrides(&pairs);
        }

        self.grid.apply_theme(&theme);
        let overrides = collect_palette_overrides(self.grid.palette().override_pairs());
        let response = ApplyThemeResponse {
            override_count: overrides.len(),
            overrides,
            foreground: format_rgba_hex(self.grid.effective_foreground()),
            background: format_rgba_hex(self.grid.effective_background()),
            cursor_color: format_rgba_hex(self.grid.effective_cursor_color()),
        };
        serde_json::to_string(&response)
            .map_err(|err| format!("failed to serialize theme state: {err}"))
    }

    /// Exports the current viewport as a CRC-32 checked `"LCVT"` v2 binary snapshot.
    pub fn export_lcvt(&self) -> Vec<u8> {
        legion_terminal_vt::encode_lcvt(&self.grid.snapshot())
    }

    /// Serializes a JSON summary of the current terminal viewport and palette state.
    pub fn snapshot_summary_json(&self) -> Result<String, String> {
        let snapshot = self.grid.snapshot();
        serialize_snapshot_summary(
            &snapshot,
            Some(self.grid.title().to_owned()),
            Some(self.grid.bell_count()),
            Some(self.grid.scrollback_len()),
        )
    }
}

fn parse_palette_input(entry: PaletteEntryInput) -> Result<(u8, Rgba), String> {
    match entry {
        PaletteEntryInput::Object { index, color } | PaletteEntryInput::Tuple(index, color) => {
            let rgba = parse_color(&color).map_err(|err| err.to_string())?;
            Ok((index, rgba))
        }
        PaletteEntryInput::Line(line) => {
            parse_palette_entry(&line).ok_or_else(|| format!("invalid palette entry `{line}`"))
        }
    }
}

fn serialize_snapshot_summary(
    snapshot: &TerminalSnapshot,
    title: Option<String>,
    bell_count: Option<u64>,
    scrollback_len: Option<usize>,
) -> Result<String, String> {
    let cols = usize::from(snapshot.cols).max(1);
    let mut lines = Vec::with_capacity(usize::from(snapshot.rows));
    let mut non_empty_cell_count = 0_usize;
    for row_cells in snapshot.cells.chunks(cols) {
        let mut line = String::new();
        for cell in row_cells {
            if cell.text.is_empty() {
                line.push(' ');
            } else {
                non_empty_cell_count += 1;
                line.push_str(&cell.text);
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    let viewport_text = lines.join("\n");
    let overrides = collect_palette_overrides(snapshot.palette.override_pairs());
    let summary = SnapshotSummaryResponse {
        cols: snapshot.cols,
        rows: snapshot.rows,
        cursor_x: snapshot.cursor_x,
        cursor_y: snapshot.cursor_y,
        cursor_visible: snapshot.cursor_visible,
        cursor_style: cursor_style_name(snapshot.cursor_style),
        cursor_blinking: snapshot.cursor_blinking,
        alternate_screen: snapshot.alternate_screen,
        title,
        bell_count,
        scrollback_len,
        foreground: format_rgba_hex(snapshot.foreground),
        background: format_rgba_hex(snapshot.background),
        cursor_color: format_rgba_hex(snapshot.cursor_color),
        palette_override_count: overrides.len(),
        palette_overrides: overrides,
        cell_count: snapshot.cells.len(),
        non_empty_cell_count,
        lines,
        viewport_text,
    };
    serde_json::to_string(&summary)
        .map_err(|err| format!("failed to serialize snapshot summary: {err}"))
}

fn collect_palette_overrides(pairs: Vec<(u8, Rgba)>) -> Vec<PaletteOverrideOut> {
    pairs
        .into_iter()
        .map(|(index, color)| PaletteOverrideOut {
            index,
            hex: format_rgba_hex(color),
            argb: color.to_argb_u32(),
        })
        .collect()
}

fn format_rgba_hex(color: Rgba) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color.r, color.g, color.b, color.a
        )
    }
}

const fn cursor_style_name(shape: GhosttyCursorShape) -> &'static str {
    match shape {
        GhosttyCursorShape::Bar => "bar",
        GhosttyCursorShape::Block => "block",
        GhosttyCursorShape::Underline => "underline",
        GhosttyCursorShape::BlockHollow => "blockHollow",
    }
}

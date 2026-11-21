//! Safe Rust bridges for `BUG-25` sparse ANSI 256-color palette and `"T3VT"` VT snapshots.

use t3_terminal_vt::palette::{AnsiPalette256, Rgba, TerminalThemeConfig};
use t3_terminal_vt::parser::{TerminalGrid, VtParser};
use t3_terminal_vt::snapshot::{decode_t3vt, encode_t3vt_v1, try_encode_t3vt};

use crate::core_types::MobileFfiError;
use crate::core_vt_types::{
    PaletteEntryOutput, PaletteOverrideInput, PaletteResolveRequest, PaletteResolveResponse,
    RgbaOutput, VtDecodedCellOutput, VtDecodedSnapshotResponse, VtRenderSnapshotRequest,
};

pub(crate) fn palette_resolve_json(input_json: &str) -> Result<String, MobileFfiError> {
    let req: PaletteResolveRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    let mut palette = AnsiPalette256::new();
    if let Some(prior) = req.prior_overrides {
        let pairs = resolve_override_pairs(prior)?;
        palette.apply_theme_overrides(&pairs);
    }

    let mut foreground = None;
    let mut background = None;
    let mut cursor_color = None;
    let mut cursor_text = None;

    if let Some(config) = req.theme_config.as_deref() {
        let parsed = TerminalThemeConfig::parse(config);
        foreground = parsed.foreground.map(RgbaOutput::from_rgba);
        background = parsed.background.map(RgbaOutput::from_rgba);
        cursor_color = parsed.cursor_color.map(RgbaOutput::from_rgba);
        cursor_text = parsed.cursor_text.map(RgbaOutput::from_rgba);
        palette.apply_theme_overrides(&parsed.palette.override_pairs());
    }

    if let Some(overrides) = req.overrides {
        let pairs = resolve_override_pairs(overrides)?;
        palette.apply_theme_overrides(&pairs);
    }

    let active_overrides: Vec<PaletteEntryOutput> = palette
        .override_pairs()
        .into_iter()
        .map(|(index, color)| PaletteEntryOutput {
            index,
            is_override: true,
            color: RgbaOutput::from_rgba(color),
        })
        .collect();

    let requested_indices: Vec<u8> = req.indices.unwrap_or_else(|| (0_u8..=255_u8).collect());
    let resolved = requested_indices
        .into_iter()
        .map(|index| PaletteEntryOutput {
            index,
            is_override: palette.override_at(index).is_some(),
            color: RgbaOutput::from_rgba(palette.resolve(index)),
        })
        .collect();

    let response = PaletteResolveResponse {
        override_count: active_overrides.len(),
        overrides: active_overrides,
        resolved,
        foreground,
        background,
        cursor_color,
        cursor_text,
    };
    serde_json::to_string(&response).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

pub(crate) fn vt_render_snapshot(input_json: &str) -> Result<Vec<u8>, MobileFfiError> {
    let req: VtRenderSnapshotRequest = serde_json::from_str(input_json)
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))?;

    let mut grid = TerminalGrid::try_new(req.cols, req.rows)?;
    if let Some(config) = req.theme_config.as_deref() {
        let theme = TerminalThemeConfig::parse(config);
        grid.apply_theme(&theme);
    }
    if let Some(overrides) = req.palette_overrides {
        let pairs = resolve_override_pairs(overrides)?;
        grid.palette_mut().apply_theme_overrides(&pairs);
    }

    let mut parser = VtParser::new();
    if let Some(raw_bytes) = req.bytes.as_deref() {
        parser.feed(raw_bytes, &mut grid);
    }
    if let Some(ansi_text) = req.ansi_text.as_deref() {
        parser.feed(ansi_text.as_bytes(), &mut grid);
    }

    let snapshot = grid.snapshot();
    match req.version.unwrap_or(2) {
        1 => Ok(encode_t3vt_v1(&snapshot)),
        2 => Ok(try_encode_t3vt(&snapshot)?),
        other => Err(MobileFfiError::InvalidInput(format!(
            "unsupported T3VT version {other}; expected 1 or 2"
        ))),
    }
}

pub(crate) fn vt_decode_snapshot_json(bytes: &[u8]) -> Result<String, MobileFfiError> {
    let snapshot = decode_t3vt(bytes)?;
    let cols = usize::from(snapshot.cols);
    let rows = usize::from(snapshot.rows);

    let palette_overrides = snapshot
        .palette
        .override_pairs()
        .into_iter()
        .map(|(index, color)| PaletteEntryOutput {
            index,
            is_override: true,
            color: RgbaOutput::from_rgba(color),
        })
        .collect();

    let mut viewport_lines = Vec::with_capacity(rows);
    for row_cells in snapshot.cells.chunks(cols.max(1)) {
        let mut line = String::with_capacity(cols);
        for cell in row_cells {
            if cell.text.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&cell.text);
            }
        }
        while line.ends_with(|ch: char| ch.is_ascii_whitespace()) {
            line.pop();
        }
        viewport_lines.push(line);
    }

    let cells = snapshot
        .cells
        .into_iter()
        .map(|cell| VtDecodedCellOutput {
            foreground: RgbaOutput::from_rgba(cell.foreground),
            background: RgbaOutput::from_rgba(cell.background),
            flags: cell.flags.bits(),
            text: cell.text,
        })
        .collect();

    let response = VtDecodedSnapshotResponse {
        cols: snapshot.cols,
        rows: snapshot.rows,
        cursor_x: snapshot.cursor_x,
        cursor_y: snapshot.cursor_y,
        cursor_visible: snapshot.cursor_visible,
        cursor_style: snapshot.cursor_style,
        cursor_blinking: snapshot.cursor_blinking,
        alternate_screen: snapshot.alternate_screen,
        foreground: RgbaOutput::from_rgba(snapshot.foreground),
        background: RgbaOutput::from_rgba(snapshot.background),
        cursor_color: RgbaOutput::from_rgba(snapshot.cursor_color),
        palette_overrides,
        cells,
        viewport_lines,
    };

    serde_json::to_string(&response).map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}

fn resolve_override_pairs(
    inputs: Vec<PaletteOverrideInput>,
) -> Result<Vec<(u8, Rgba)>, MobileFfiError> {
    let mut pairs = Vec::with_capacity(inputs.len());
    for entry in inputs {
        pairs.push(entry.into_pair()?);
    }
    Ok(pairs)
}

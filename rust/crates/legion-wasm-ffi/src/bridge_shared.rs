//! Pure-Rust host-testable bridge helpers for [`legion_shared`] (`frame_parts` and `search`).

use legion_shared::frame_parts::{
    SOCKET_FRAME_PART_BYTES, split_socket_frame_utf16, split_socket_frame_with_limit,
    utf16_wire_utf8_len,
};
use legion_shared::search::{
    FieldMatchMode, FieldOptions, FuzzyPaletteIndex, PaletteEntry, PaletteField, PaletteMatch,
    SearchOptions, fold_search_query,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SplitFramePartsRequest {
    #[serde(default, alias = "utf16_units", alias = "units", alias = "codeUnits")]
    utf16_units: Option<Vec<u16>>,
    #[serde(default, alias = "frame", alias = "payload")]
    text: Option<String>,
    #[serde(default, alias = "part_bytes", alias = "limitBytes")]
    part_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SplitFramePartsResponse {
    part_count: usize,
    wire_utf8_bytes: usize,
    part_bytes_limit: usize,
    parts: Vec<String>,
}

/// Splits a UTF-16 code-unit array or UTF-8 string into WebSocket `?frameParts` chunks.
pub fn split_utf16_frame_parts_json(input_json: &str) -> Result<String, String> {
    let req: SplitFramePartsRequest = serde_json::from_str(input_json)
        .map_err(|err| format!("invalid frame parts JSON: {err}"))?;
    let part_bytes = req.part_bytes.unwrap_or(SOCKET_FRAME_PART_BYTES);

    let (parts, wire_utf8_bytes) = if let Some(units) = req.utf16_units {
        let wire_len = utf16_wire_utf8_len(&units);
        let split = split_socket_frame_utf16(&units, part_bytes).map_err(|err| err.to_string())?;
        (split, wire_len)
    } else if let Some(text) = req.text {
        let wire_len = text.len();
        let split =
            split_socket_frame_with_limit(&text, part_bytes).map_err(|err| err.to_string())?;
        (split, wire_len)
    } else {
        return Err("split_utf16_frame_parts_json requires `utf16Units` or `text`".to_owned());
    };

    let response = SplitFramePartsResponse {
        part_count: parts.len(),
        wire_utf8_bytes,
        part_bytes_limit: part_bytes,
        parts,
    };
    serde_json::to_string(&response).map_err(|err| format!("failed to serialize parts: {err}"))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FieldModeInput {
    #[serde(alias = "word_start")]
    WordStart,
    #[serde(alias = "mid_word")]
    MidWord,
    #[serde(alias = "whole_words")]
    WholeWords,
    #[serde(alias = "keyword_word_start")]
    KeywordWordStart,
    #[serde(alias = "keyword_whole_words")]
    KeywordWholeWords,
}

impl From<FieldModeInput> for FieldMatchMode {
    fn from(value: FieldModeInput) -> Self {
        match value {
            FieldModeInput::WordStart => Self::WordStart,
            FieldModeInput::MidWord => Self::MidWord,
            FieldModeInput::WholeWords => Self::WholeWords,
            FieldModeInput::KeywordWordStart => Self::KeywordWordStart,
            FieldModeInput::KeywordWholeWords => Self::KeywordWholeWords,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaletteFieldInput {
    text: String,
    #[serde(default = "default_unit_weight")]
    weight: f64,
    #[serde(default)]
    mode: Option<FieldModeInput>,
    #[serde(default)]
    context: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaletteEntryInput {
    fields: Vec<PaletteFieldInput>,
    #[serde(default = "default_unit_weight", alias = "usage_boost")]
    usage_boost: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaletteSearchRequest {
    query: String,
    #[serde(default, alias = "items")]
    titles: Option<Vec<String>>,
    #[serde(default)]
    entries: Option<Vec<PaletteEntryInput>>,
    #[serde(default = "default_true", alias = "allow_typos")]
    allow_typos: bool,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchRangeOut {
    start: usize,
    end: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaletteMatchOut {
    item_index: usize,
    score: f64,
    quality: f64,
    whole_words: bool,
    strong: bool,
    ranges: Vec<SearchRangeOut>,
}

impl From<PaletteMatch> for PaletteMatchOut {
    fn from(m: PaletteMatch) -> Self {
        Self {
            item_index: m.item_index,
            score: m.score,
            quality: m.quality,
            whole_words: m.whole_words,
            strong: m.strong,
            ranges: m
                .ranges
                .into_iter()
                .map(|r| SearchRangeOut {
                    start: r.start,
                    end: r.end,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaletteSearchResponse {
    query: String,
    normalized_query: String,
    total_indexed: usize,
    matches: Vec<PaletteMatchOut>,
}

/// Runs `SoA` fuzzy command-palette search over `titles` or weighted multi-field `entries`.
pub fn palette_search_json(input_json: &str) -> Result<String, String> {
    let req: PaletteSearchRequest = serde_json::from_str(input_json)
        .map_err(|err| format!("invalid palette search JSON: {err}"))?;

    let index = if let Some(entries) = &req.entries {
        let borrowed: Vec<PaletteEntry<'_>> = entries
            .iter()
            .map(|entry| PaletteEntry {
                fields: entry
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(slot, field)| PaletteField {
                        text: &field.text,
                        options: FieldOptions {
                            weight: field.weight,
                            mode: field.mode.map_or_else(
                                || {
                                    if slot == 0 {
                                        FieldMatchMode::MidWord
                                    } else {
                                        FieldMatchMode::WordStart
                                    }
                                },
                                Into::into,
                            ),
                            context: field.context,
                        },
                    })
                    .collect(),
                usage_boost: entry.usage_boost,
            })
            .collect();
        FuzzyPaletteIndex::from_entries(borrowed)
    } else if let Some(titles) = &req.titles {
        FuzzyPaletteIndex::new(titles)
    } else {
        return Err("palette_search_json requires `titles` or `entries`".to_owned());
    };

    let options = SearchOptions {
        allow_typos: req.allow_typos,
        limit: req.limit,
    };
    let matches = index
        .search_with_options(&req.query, options)
        .into_iter()
        .map(PaletteMatchOut::from)
        .collect();
    let response = PaletteSearchResponse {
        normalized_query: fold_search_query(&req.query),
        query: req.query,
        total_indexed: index.len(),
        matches,
    };
    serde_json::to_string(&response)
        .map_err(|err| format!("failed to serialize palette matches: {err}"))
}

const fn default_true() -> bool {
    true
}

const fn default_unit_weight() -> f64 {
    1.0
}

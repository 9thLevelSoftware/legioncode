//! Streaming terminal history sanitizer filtering replayable queries (`982bbcdfe1` #40149).
//!
//! Strips `OSC 4` palette queries, `OSC 10/11/12` dynamic color queries/replies, `DCS`
//! (`DECRQSS`/`XTGETTCAP`) queries, and `CSI` (`DA`/`DSR`/`DECRQM`/`XTVERSION`/Kitty)
//! queries across arbitrary chunk boundaries so replaying recorded terminal history never
//! triggers terminal response feedback loops.

use serde::{Deserialize, Serialize};

use crate::history_sanitizer_scan::{
    c1_effect, find_escape_sequence_end, find_string_end, floor_char_boundary, is_cancel_control,
    is_csi_final, strip_string_terminator, terminated_osc_or_dcs,
};

/// Result of feeding one raw PTY output chunk into [`TerminalHistorySanitizer`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizedHistoryChunk {
    /// Sanitized text safe to append to recorded terminal history.
    pub visible_text: String,
    /// Incomplete control sequence buffered at the end of the chunk until more bytes arrive.
    pub pending_control_sequence: String,
}

/// Streaming sanitizer preserving incomplete control sequences across chunk boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalHistorySanitizer {
    pending_control_sequence: String,
}

impl TerminalHistorySanitizer {
    /// Creates an empty sanitizer ready to ingest PTY UTF-8 chunks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns any currently buffered incomplete control sequence.
    pub fn pending(&self) -> &str {
        &self.pending_control_sequence
    }

    /// Sanitizes one raw PTY chunk, carrying over any incomplete trailing sequence.
    pub fn push_chunk(&mut self, data: &str) -> SanitizedHistoryChunk {
        let out = sanitize_terminal_history_chunk(&self.pending_control_sequence, data);
        self.pending_control_sequence
            .clone_from(&out.pending_control_sequence);
        out
    }

    /// Flushes and clears any incomplete trailing control sequence at stream end.
    pub fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending_control_sequence)
    }
}

/// Sanitizes a single complete history string in one pass.
pub fn sanitize_terminal_history(input: &str) -> String {
    let mut sanitizer = TerminalHistorySanitizer::new();
    let mut out = sanitizer.push_chunk(input).visible_text;
    out.push_str(&sanitizer.finish());
    out
}

/// Returns `true` when `content` is an `OSC 4` palette query whose every color spec is `"?"`.
///
/// Mirrors `isOscPaletteQuery` in `apps/server/src/terminal/Manager.ts` (`#40149`): skips C0
/// controls (`0x00..=0x1F`) inside the OSC string and ignores empty `;` fields, while leaving
/// mixed setter+query `OSC 4` sequences (such as `4;1;red;2;?`) intact.
pub fn is_osc_palette_query(content: &str) -> bool {
    let after_c0 = content.trim_start_matches(|ch: char| ch <= '\x1f');
    if !after_c0.starts_with('4') || !content.contains('?') {
        return false;
    }
    let cleaned: String = content.chars().filter(|&ch| ch > '\x1f').collect();
    let mut fields = cleaned.split(';').filter(|field| !field.is_empty());
    let Some(code) = fields.next() else {
        return false;
    };
    if code != "4" {
        return false;
    }
    let mut pair_field_count = 0_usize;
    for (idx, field) in fields.enumerate() {
        pair_field_count += 1;
        if idx % 2 == 1 && field != "?" {
            return false;
        }
    }
    pair_field_count > 1
}

/// Returns `true` for `OSC 10/11/12` dynamic color queries (`?`) and replies (`rgb:...`).
pub fn should_strip_osc_sequence(content: &str) -> bool {
    let Some((code, rest)) = content.split_once(';') else {
        return false;
    };
    matches!(code, "10" | "11" | "12") && (rest.starts_with('?') || rest.starts_with("rgb:"))
}

/// Returns `true` for `DECRQSS` and `XTGETTCAP` `DCS` query/reply strings.
pub fn should_strip_dcs_sequence(content: &str) -> bool {
    let rest = content.strip_prefix(['0', '1']).unwrap_or(content);
    let mut chars = rest.chars();
    matches!(chars.next(), Some('$' | '+')) && matches!(chars.next(), Some('q' | 'r'))
}

/// Returns `true` for replayable `CSI` device-attribute, cursor-report, and mode queries.
pub fn should_strip_csi_sequence(body: &str, final_byte: char) -> bool {
    match final_byte {
        'n' => true,
        'R' => body
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b';' | b'?')),
        'c' => body
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'>' | b'=' | b';' | b'?')),
        'p' | 'y' => body.strip_suffix('$').is_some_and(|head| {
            head.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b';' | b'?'))
        }),
        'q' => body
            .strip_prefix('>')
            .is_some_and(|tail| tail.bytes().all(|b| b.is_ascii_digit() || b == b';')),
        'u' => body.starts_with('?'),
        _ => false,
    }
}

/// Stateless chunk-level implementation matching `sanitizeTerminalHistoryChunk` in `Manager.ts`.
pub fn sanitize_terminal_history_chunk(
    pending_control_sequence: &str,
    data: &str,
) -> SanitizedHistoryChunk {
    if pending_control_sequence.is_empty() && !data.contains('\x1b') {
        return SanitizedHistoryChunk {
            visible_text: data.to_owned(),
            pending_control_sequence: String::new(),
        };
    }

    let input = format!("{pending_control_sequence}{data}");
    let pending_scan_start = pending_control_sequence.len().saturating_sub(1);
    let mut visible_text = String::with_capacity(input.len());
    let mut index = 0_usize;
    let mut c1_introducer = String::new();

    while index < input.len() {
        let rest = &input[index..];
        if !c1_introducer.is_empty() || rest.starts_with('\x1b') {
            let prefix = std::mem::take(&mut c1_introducer);
            let content_start = if prefix.is_empty() { index + 2 } else { index };
            let next_byte = if prefix.is_empty() {
                input.as_bytes().get(index + 1).copied()
            } else {
                prefix.as_bytes().get(1).copied()
            };
            let Some(next_code) = next_byte else {
                return SanitizedHistoryChunk {
                    visible_text,
                    pending_control_sequence: input[index..].to_owned(),
                };
            };

            match next_code {
                b'[' => {
                    match process_csi(
                        &input,
                        index,
                        content_start,
                        &prefix,
                        &mut visible_text,
                        &mut c1_introducer,
                    ) {
                        StepOutcome::Advance(next_idx) => index = next_idx,
                        StepOutcome::Incomplete => {
                            return SanitizedHistoryChunk {
                                visible_text,
                                pending_control_sequence: format!("{prefix}{}", &input[index..]),
                            };
                        }
                    }
                    continue;
                }
                b']' | b'P' | b'^' | b'_' => {
                    match process_string_sequence(
                        &input,
                        index,
                        content_start,
                        pending_scan_start,
                        &prefix,
                        next_code,
                        &mut visible_text,
                        &mut c1_introducer,
                    ) {
                        StepOutcome::Advance(next_idx) => index = next_idx,
                        StepOutcome::Incomplete => {
                            return SanitizedHistoryChunk {
                                visible_text,
                                pending_control_sequence: format!("{prefix}{}", &input[index..]),
                            };
                        }
                    }
                    continue;
                }
                _ => {
                    let Some(esc_end) = find_escape_sequence_end(&input, index + 1) else {
                        return SanitizedHistoryChunk {
                            visible_text,
                            pending_control_sequence: input[index..].to_owned(),
                        };
                    };
                    // `ESC Z` (DECID) is the legacy primary device attributes query.
                    if next_code != b'Z' {
                        visible_text.push_str(&input[index..esc_end]);
                    }
                    index = esc_end;
                    continue;
                }
            }
        }

        let next_esc = input[index + 1..]
            .find('\x1b')
            .map_or(input.len(), |rel| index + 1 + rel);
        visible_text.push_str(&input[index..next_esc]);
        index = next_esc;
    }

    SanitizedHistoryChunk {
        visible_text,
        pending_control_sequence: c1_introducer,
    }
}

enum StepOutcome {
    Advance(usize),
    Incomplete,
}

#[expect(
    clippy::too_many_arguments,
    reason = "shares chunk cursor and introducer state with process_csi"
)]
fn process_string_sequence(
    input: &str,
    seq_start: usize,
    content_start: usize,
    pending_scan_start: usize,
    prefix: &str,
    next_code: u8,
    visible_text: &mut String,
    c1_introducer: &mut String,
) -> StepOutcome {
    let is_osc = next_code == b']';
    let scan_from = floor_char_boundary(input, content_start.max(pending_scan_start));
    let Some(string_end) = find_string_end(input, scan_from, is_osc) else {
        return StepOutcome::Incomplete;
    };
    let end = string_end.end;
    let content_end = string_end.c1.as_ref().map_or(end, |hit| hit.index);
    let prefix_tail = prefix.get(2..).unwrap_or("");
    let raw_content = format!("{prefix_tail}{}", &input[content_start..content_end]);
    let content = strip_string_terminator(&raw_content);

    if is_osc && is_osc_palette_query(content) && terminated_osc_or_dcs(&input[seq_start..end]) {
        // ST ends what the query's ESC cut short and leaves ground state cleanly.
        visible_text.push_str("\x1b\\");
        return StepOutcome::Advance(end);
    }

    let strip = (is_osc && should_strip_osc_sequence(content))
        || (next_code == b'P' && should_strip_dcs_sequence(content));
    match string_end.c1 {
        None => {
            if !strip {
                visible_text.push_str(prefix);
                visible_text.push_str(&input[seq_start..end]);
            }
        }
        Some(hit) => {
            if !strip && hit.effect.next.is_empty() && !hit.effect.query {
                visible_text.push_str(prefix);
                visible_text.push_str(&input[seq_start..end]);
            } else {
                if !strip {
                    visible_text.push_str(prefix);
                    visible_text.push_str(&input[seq_start..hit.index]);
                    visible_text.push('\u{009C}');
                }
                visible_text.push_str(&hit.effect.safe);
                *c1_introducer = hit.effect.next;
            }
        }
    }
    StepOutcome::Advance(end)
}

fn process_csi(
    input: &str,
    seq_start: usize,
    content_start: usize,
    prefix: &str,
    visible_text: &mut String,
    c1_introducer: &mut String,
) -> StepOutcome {
    let mut body = String::new();
    let mut executed_controls = String::new();
    for (rel, ch) in input[content_start..].char_indices() {
        let cursor = content_start + rel;
        let cancelled = is_cancel_control(ch);
        if cancelled || is_csi_final(ch) {
            if cancelled || !should_strip_csi_sequence(&body, ch) {
                visible_text.push_str(prefix);
                visible_text.push_str(&input[seq_start..=cursor]);
            } else {
                visible_text.push_str(&executed_controls);
            }
            return StepOutcome::Advance(cursor + 1);
        }
        if ch as u32 >= 0x80 {
            let end = cursor + ch.len_utf8();
            if let Some(effect) = c1_effect(ch) {
                if effect.next.is_empty() && !effect.query {
                    visible_text.push_str(prefix);
                    visible_text.push_str(&input[seq_start..end]);
                } else {
                    visible_text.push_str(&executed_controls);
                    visible_text.push_str(&effect.safe);
                }
                *c1_introducer = effect.next;
                return StepOutcome::Advance(end);
            }
            continue;
        }
        if ch == '\x1b' {
            visible_text.push_str(&executed_controls);
            return StepOutcome::Advance(cursor);
        }
        if (ch as u32) < 0x10 {
            executed_controls.push(ch);
        } else if (ch as u32) >= 0x20 && ch != '\x7f' {
            body.push(ch);
        }
    }
    StepOutcome::Incomplete
}

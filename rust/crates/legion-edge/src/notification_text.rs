//! Emoji-safe notification text truncation shared across relays and desktop/mobile surfaces
//! (`#29830`).

/// Default notification summary / title UTF-16 code-unit ceiling (`120`, matching APNs banners).
pub const MAX_NOTIFICATION_SUMMARY_UTF16_LEN: usize = 120;

const ELLIPSIS: &str = "...";
const ELLIPSIS_UTF16_LEN: usize = 3;

/// Truncates `value` to at most `max_utf16_len` UTF-16 code units without ever splitting a UTF-16
/// surrogate pair, multi-byte UTF-8 scalar, variation selector, regional flag pair, skin-tone
/// modifier, combining mark, or ZWJ emoji sequence.
pub fn truncate_notification_text(value: &str, max_utf16_len: usize) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || max_utf16_len == 0 {
        return String::new();
    }
    if utf16_len(trimmed) <= max_utf16_len {
        return trimmed.to_owned();
    }
    if max_utf16_len <= ELLIPSIS_UTF16_LEN {
        return take_utf16_prefix(trimmed, max_utf16_len).to_owned();
    }
    let budget = max_utf16_len - ELLIPSIS_UTF16_LEN;
    let prefix = take_utf16_prefix(trimmed, budget).trim_end();
    format!("{prefix}{ELLIPSIS}")
}

/// Truncates a notification title / summary using [`MAX_NOTIFICATION_SUMMARY_UTF16_LEN`].
pub fn truncate_notification_title(title: &str) -> String {
    truncate_notification_text(title, MAX_NOTIFICATION_SUMMARY_UTF16_LEN)
}

fn utf16_len(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

/// Returns the longest prefix of `value` whose UTF-16 length is `<= max_utf16_len` and whose end
/// does not fall inside a joined emoji, modifier, regional indicator, or combining sequence.
fn take_utf16_prefix(value: &str, max_utf16_len: usize) -> &str {
    let clusters = cluster_byte_ranges(value);
    let mut used_utf16 = 0usize;
    let mut end_byte = 0usize;
    for (start, end) in clusters {
        let cluster_utf16 = utf16_len(&value[start..end]);
        if used_utf16 + cluster_utf16 > max_utf16_len {
            break;
        }
        used_utf16 += cluster_utf16;
        end_byte = end;
    }
    &value[..end_byte]
}

fn cluster_byte_ranges(value: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    let mut ranges = Vec::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        let start = chars[idx].0;
        let first = chars[idx].1;
        idx += 1;
        let mut after_zwj = false;
        let mut regional_pair = is_regional_indicator(first);
        while idx < chars.len() {
            let next = chars[idx].1;
            if after_zwj {
                after_zwj = false;
                idx += 1;
                continue;
            }
            if regional_pair && is_regional_indicator(next) {
                regional_pair = false;
                idx += 1;
                continue;
            }
            regional_pair = false;
            if is_cluster_extender(next) {
                after_zwj = next == '\u{200D}';
                idx += 1;
                continue;
            }
            break;
        }
        let end = chars.get(idx).map_or(value.len(), |(offset, _)| *offset);
        ranges.push((start, end));
    }
    ranges
}

fn is_regional_indicator(ch: char) -> bool {
    matches!(ch as u32, 0x1F1E6..=0x1F1FF)
}

fn is_cluster_extender(ch: char) -> bool {
    let cp = ch as u32;
    ch == '\u{200D}'
        || matches!(
            cp,
            0x0300..=0x036F
                | 0x1AB0..=0x1AFF
                | 0x1DC0..=0x1DFF
                | 0x20D0..=0x20FF
                | 0xFE00..=0xFE0F
                | 0xFE20..=0xFE2F
                | 0x1F3FB..=0x1F3FF
                | 0xE0020..=0xE007F
                | 0xE0100..=0xE01EF
        )
}

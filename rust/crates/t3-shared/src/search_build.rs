//! `SoA` arena encoding and Unicode word-boundary bitmask construction for [`crate::search`].

use crate::search::{FieldMatchMode, PaletteField};
use crate::unicode_so_table::is_symbol_other;
use crate::utf16::fold_char_in_place;

pub(crate) const FLAG_PRIMARY: u8 = 1 << 0;
pub(crate) const FLAG_WHOLE_WORDS: u8 = 1 << 1;
pub(crate) const FLAG_KEYWORD: u8 = 1 << 2;
pub(crate) const FLAG_CONTEXT: u8 = 1 << 3;
pub(crate) const FLAG_MID_WORD: u8 = 1 << 4;

pub(crate) struct EncodedFieldMeta {
    pub(crate) text_offset: u32,
    pub(crate) text_len: u32,
    pub(crate) mask_offset: u32,
    pub(crate) weight: f64,
    pub(crate) flags: u8,
}

pub(crate) fn encode_field_into_arenas(
    is_primary: bool,
    field: &PaletteField<'_>,
    folded_utf16: &mut Vec<u16>,
    word_start_masks: &mut Vec<u64>,
    alnum_masks: &mut Vec<u64>,
) -> EncodedFieldMeta {
    let text_offset = u32::try_from(folded_utf16.len()).unwrap_or(u32::MAX);
    let mask_offset = u32::try_from(word_start_masks.len()).unwrap_or(u32::MAX);
    let mut u16_buf = [0u16; 2];

    let chars: Vec<char> = field.text.chars().collect();
    let mut u16_len = 0usize;
    for &ch in &chars {
        let folded = fold_char_in_place(ch);
        let units = folded.encode_utf16(&mut u16_buf);
        folded_utf16.extend_from_slice(units);
        u16_len += units.len();
    }

    let mask_words = u16_len.div_ceil(64);
    word_start_masks.resize(word_start_masks.len() + mask_words, 0);
    alnum_masks.resize(alnum_masks.len() + mask_words, 0);

    let mask_base = mask_offset as usize;
    let mut u16_cursor = 0usize;
    for (pos, &ch) in chars.iter().enumerate() {
        let prev = pos.checked_sub(1).and_then(|idx| chars.get(idx).copied());
        let before_prev = pos.checked_sub(2).and_then(|idx| chars.get(idx).copied());
        let next = chars.get(pos + 1).copied();
        if is_word_start_char(before_prev, prev, ch, next) {
            set_bit(&mut word_start_masks[mask_base..], u16_cursor);
        }
        let ch_u16_len = ch.len_utf16();
        if ch.is_alphanumeric() {
            for offset in 0..ch_u16_len {
                set_bit(&mut alnum_masks[mask_base..], u16_cursor + offset);
            }
        }
        u16_cursor += ch_u16_len;
    }

    let mut flags = 0u8;
    if is_primary {
        flags |= FLAG_PRIMARY;
    }
    match field.options.mode {
        FieldMatchMode::WordStart => {}
        FieldMatchMode::MidWord => flags |= FLAG_MID_WORD,
        FieldMatchMode::WholeWords => flags |= FLAG_WHOLE_WORDS,
        FieldMatchMode::KeywordWordStart => flags |= FLAG_KEYWORD,
        FieldMatchMode::KeywordWholeWords => flags |= FLAG_KEYWORD | FLAG_WHOLE_WORDS,
    }
    if field.options.context {
        flags |= FLAG_CONTEXT;
    }

    EncodedFieldMeta {
        text_offset,
        text_len: u32::try_from(u16_len).unwrap_or(u32::MAX),
        mask_offset,
        weight: field.options.weight,
        flags,
    }
}

#[inline]
fn set_bit(words: &mut [u64], bit_index: usize) {
    words[bit_index >> 6] |= 1u64 << (bit_index & 63);
}

#[inline]
fn is_apostrophe(ch: char) -> bool {
    matches!(ch, '\'' | '\u{2018}' | '\u{2019}')
}

#[inline]
fn is_cjk(ch: char) -> bool {
    let cp = ch as u32;
    matches!(
        cp,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn is_word_start_char(
    before_prev: Option<char>,
    prev: Option<char>,
    ch: char,
    next: Option<char>,
) -> bool {
    if !(ch.is_alphanumeric() || is_symbol_other(ch)) {
        return false;
    }
    let after_apostrophe =
        prev.is_some_and(is_apostrophe) && before_prev.is_some_and(char::is_alphanumeric);
    if after_apostrophe {
        return false;
    }
    let Some(p) = prev else {
        return true;
    };
    !p.is_alphanumeric()
        || (p.is_lowercase() && ch.is_uppercase())
        || (p.is_uppercase() && ch.is_uppercase() && next.is_some_and(char::is_lowercase))
        || (p.is_numeric() != ch.is_numeric())
        || is_cjk(ch)
}

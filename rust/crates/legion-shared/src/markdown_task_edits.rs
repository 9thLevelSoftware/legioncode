//! Markdown task-list checkbox offset lookup and toggling (`markdownTaskEdits.ts`).

/// Returns the byte offset of `[ ]`, `[x]`, or `[X]` on the first line of the list item at `list_item_start`.
#[must_use]
pub fn task_list_marker_offset(markdown: &str, list_item_start: usize) -> Option<usize> {
    let rest = markdown.get(list_item_start..)?;
    let first_line = rest.split('\n').next().unwrap_or(rest);
    let trimmed_len = first_line
        .bytes()
        .take_while(u8::is_ascii_whitespace)
        .count();
    let after_indent = &first_line[trimmed_len..];
    let after_bullet = match after_indent.as_bytes().first().copied()? {
        b'-' | b'+' | b'*' => &after_indent[1..],
        b'0'..=b'9' => {
            let digits = after_indent.bytes().take_while(u8::is_ascii_digit).count();
            let after_digits = &after_indent[digits..];
            after_digits
                .strip_prefix('.')
                .or_else(|| after_digits.strip_prefix(')'))?
        }
        _ => return None,
    };
    let space_after_bullet = after_bullet
        .bytes()
        .take_while(u8::is_ascii_whitespace)
        .count();
    if space_after_bullet == 0 {
        return None;
    }
    let marker = &after_bullet[space_after_bullet..];
    let bytes = marker.as_bytes();
    if bytes.len() >= 3
        && bytes[0] == b'['
        && matches!(bytes[1], b' ' | b'x' | b'X')
        && bytes[2] == b']'
    {
        Some(list_item_start + (first_line.len() - marker.len()))
    } else {
        None
    }
}

/// Toggles the checkbox character at byte offset `marker_offset`, returning `markdown` unchanged if invalid.
#[must_use]
pub fn set_markdown_task_checked(markdown: &str, marker_offset: usize, checked: bool) -> String {
    let bytes = markdown.as_bytes();
    if marker_offset
        .checked_add(2)
        .is_none_or(|end| end >= bytes.len())
        || !markdown.is_char_boundary(marker_offset)
        || bytes[marker_offset] != b'['
        || !matches!(bytes[marker_offset + 1], b' ' | b'x' | b'X')
        || bytes[marker_offset + 2] != b']'
    {
        return markdown.to_owned();
    }
    let mark = if checked { "x" } else { " " };
    format!(
        "{}{}{}",
        &markdown[..=marker_offset],
        mark,
        &markdown[marker_offset + 2..]
    )
}

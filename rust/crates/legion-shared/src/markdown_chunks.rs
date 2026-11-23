//! Top-level block chunking for incremental markdown rendering (`markdownChunks.ts`).

/// Default target chunk size in UTF-16 code units (`CHUNK_TARGET_CHARS`).
pub const DEFAULT_MARKDOWN_CHUNK_TARGET_CHARS: usize = 4 * 1024;
/// Maximum single-chunk length past which callers show raw source (`LARGE_MARKDOWN_CHUNK_MAX_CHARS`).
pub const LARGE_MARKDOWN_CHUNK_MAX_CHARS: usize = 128 * 1024;
const FRONTMATTER_MAX_LINES: usize = 1000;
const SHARED_DEFINITIONS_MAX_CHARS: usize = 16 * 1024;

/// A bounded slice of top-level markdown lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownChunk {
    /// Raw markdown chunk text joined with `\n`.
    pub text: String,
    /// Number of `\n`-delimited lines inside this chunk.
    pub lines: usize,
}

/// Output produced by [`split_markdown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitMarkdown {
    /// Ordered top-level chunks preserving input offsets.
    pub chunks: Vec<MarkdownChunk>,
    /// Shared link reference definitions collected across the document.
    pub definitions: String,
}

#[derive(Clone, Copy)]
enum BlockCloser {
    Fence { ch: u8, len: usize },
    UntilSubstring(&'static str),
    UntilTagCI(&'static str),
    UntilGt,
    UntilBlankLine,
    Frontmatter { seen: usize },
}

impl BlockCloser {
    fn check(&mut self, line: &str) -> bool {
        match self {
            Self::Fence { ch, len } => fence_prefix(line).is_some_and(|(c, l)| {
                c == *ch
                    && l >= *len
                    && line.trim().as_bytes().get(l..).is_none_or(<[u8]>::is_empty)
            }),
            Self::UntilSubstring(needle) => line.contains(*needle),
            Self::UntilTagCI(needle) => line.to_ascii_lowercase().contains(*needle),
            Self::UntilGt => line.contains('>'),
            Self::UntilBlankLine => line.trim().is_empty(),
            Self::Frontmatter { seen } => {
                *seen += 1;
                is_frontmatter_close(line) || *seen >= FRONTMATTER_MAX_LINES
            }
        }
    }
}

fn strip_trailing_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

fn skip_up_to_3_spaces(line: &str) -> &str {
    let bytes = line.as_bytes();
    let spaces = bytes.iter().take(3).take_while(|&&b| b == b' ').count();
    &line[spaces..]
}

fn fence_prefix(line: &str) -> Option<(u8, usize)> {
    let s = skip_up_to_3_spaces(line);
    let first = *s.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = s.bytes().take_while(|&b| b == first).count();
    if len < 3 {
        return None;
    }
    let info = strip_trailing_cr(&s[len..]);
    if first == b'`' && info.contains('`') {
        return None;
    }
    Some((first, len))
}

fn is_atx_heading(line: &str) -> bool {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let rest = &line[hashes..];
    rest.is_empty() || rest.starts_with([' ', '\t', '\r'])
}

fn is_thematic_break(line: &str) -> bool {
    let s = strip_trailing_cr(skip_up_to_3_spaces(line));
    let Some(marker) = s
        .bytes()
        .next()
        .filter(|&b| matches!(b, b'-' | b'*' | b'_'))
    else {
        return false;
    };
    let mut count = 0usize;
    for b in s.bytes() {
        if b == marker {
            count += 1;
        } else if b != b' ' && b != b'\t' {
            return false;
        }
    }
    count >= 3
}

fn is_frontmatter_open(line: &str) -> bool {
    let s = line.strip_prefix('\u{FEFF}').unwrap_or(line);
    let s = strip_trailing_cr(s);
    let Some(rest) = s.strip_prefix("---") else {
        return false;
    };
    rest.bytes().all(|b| b == b' ' || b == b'\t')
}

fn is_frontmatter_close(line: &str) -> bool {
    let s = strip_trailing_cr(line);
    let Some(rest) = s.strip_prefix("---").or_else(|| s.strip_prefix("...")) else {
        return false;
    };
    rest.bytes().all(|b| b == b' ' || b == b'\t')
}

fn html_tag_start(line: &str) -> bool {
    let s = skip_up_to_3_spaces(line);
    let Some(s) = s.strip_prefix('<') else {
        return false;
    };
    let s = s.strip_prefix('/').unwrap_or(s);
    let Some(first) = s.bytes().next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let name_len = s
        .bytes()
        .take_while(|&b| b.is_ascii_alphanumeric() || b == b'-')
        .count();
    let after = &s[name_len..];
    after.is_empty() || after.starts_with(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
}

fn block_closer(line: &str) -> Option<BlockCloser> {
    if let Some((ch, len)) = fence_prefix(line) {
        return Some(BlockCloser::Fence { ch, len });
    }
    let s = skip_up_to_3_spaces(line);
    let lower = s.to_ascii_lowercase();
    for tag in ["script", "pre", "style", "textarea"] {
        if let Some(after) = lower.strip_prefix('<').and_then(|r| r.strip_prefix(tag)) {
            let boundary = after.is_empty()
                || after.starts_with(|c: char| c.is_ascii_whitespace() || c == '>');
            if boundary {
                let closer = match tag {
                    "script" => "</script>",
                    "pre" => "</pre>",
                    "style" => "</style>",
                    _ => "</textarea>",
                };
                return if after.contains(closer) {
                    None
                } else {
                    Some(BlockCloser::UntilTagCI(closer))
                };
            }
        }
    }
    if let Some(after) = s.strip_prefix("<!--") {
        return (!after.contains("-->")).then_some(BlockCloser::UntilSubstring("-->"));
    }
    if let Some(after) = s.strip_prefix("<?") {
        return (!after.contains("?>")).then_some(BlockCloser::UntilSubstring("?>"));
    }
    if let Some(after) = s.strip_prefix("<![CDATA[") {
        return (!after.contains("]]>")).then_some(BlockCloser::UntilSubstring("]]>"));
    }
    if s.starts_with("<!") && s.as_bytes().get(2).is_some_and(u8::is_ascii_alphabetic) {
        return (!s[3..].contains('>')).then_some(BlockCloser::UntilGt);
    }
    if let Some(after) = s.strip_prefix("$$") {
        return (!after.contains("$$")).then_some(BlockCloser::UntilSubstring("$$"));
    }
    html_tag_start(line).then_some(BlockCloser::UntilBlankLine)
}

fn is_ordered_item(line: &str) -> bool {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=9).contains(&digits) {
        return false;
    }
    let rest = &line[digits..];
    let Some(after) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) else {
        return false;
    };
    after.is_empty() || after.starts_with([' ', '\t', '\r'])
}

fn is_block_start(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    first.is_alphanumeric() || matches!(first, '`' | '[' | '!' | '|' | '>')
}

fn is_link_definition(line: &str) -> bool {
    let s = strip_trailing_cr(skip_up_to_3_spaces(line));
    let Some(rest) = s.strip_prefix('[') else {
        return false;
    };
    if rest.starts_with('^') {
        return false;
    }
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b']' {
            break;
        }
        i += 1;
    }
    if i == 0 || !rest[i..].starts_with("]:") {
        return false;
    }
    let after_colon = rest[i + 2..].trim_start_matches([' ', '\t']);
    if after_colon.is_empty() {
        return false;
    }
    let dest_rest = if let Some(inner) = after_colon.strip_prefix('<') {
        let Some(end) = inner.find('>') else {
            return false;
        };
        if inner[..end].contains(['<', '\n']) {
            return false;
        }
        &inner[end + 1..]
    } else if after_colon.starts_with('<') {
        return false;
    } else {
        let len = after_colon
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(after_colon.len());
        &after_colon[len..]
    };
    let tail = dest_rest.trim_start_matches([' ', '\t']);
    if tail.is_empty() {
        return true;
    }
    if dest_rest.len() == tail.len() {
        return false;
    }
    let (open, close) = match tail.as_bytes()[0] {
        b'"' => (b'"', b'"'),
        b'\'' => (b'\'', b'\''),
        b'(' => (b'(', b')'),
        _ => return false,
    };
    let tbytes = &tail.as_bytes()[1..];
    let mut j = 0usize;
    while j < tbytes.len() {
        if tbytes[j] == b'\\' && j + 1 < tbytes.len() {
            j += 2;
            continue;
        }
        if tbytes[j] == close {
            return tbytes[j + 1..].iter().all(|&b| b == b' ' || b == b'\t');
        }
        if open == b'(' && tbytes[j] == b'(' {
            return false;
        }
        j += 1;
    }
    false
}

/// Splits `text` into top-level markdown chunks of roughly `target_chars` UTF-16 code units.
#[must_use]
pub fn split_markdown(text: &str, target_chars: usize) -> SplitMarkdown {
    let mut chunks = Vec::new();
    let mut definitions = Vec::new();
    let mut lines: Vec<&str> = Vec::new();
    let mut first = true;
    let mut chars = 0usize;
    let mut closer: Option<BlockCloser> = None;
    let mut after_blank_line = false;
    let mut after_definition = false;
    let mut after_heading = false;
    let mut definition_chars = 0usize;

    let flush = |chunks: &mut Vec<MarkdownChunk>, lines: &mut Vec<&str>, chars: &mut usize| {
        chunks.push(MarkdownChunk {
            text: lines.join("\n"),
            lines: lines.len(),
        });
        lines.clear();
        *chars = 0;
    };

    for line in text.split('\n') {
        if let Some(mut active) = closer {
            if active.check(line) {
                closer = None;
            } else {
                closer = Some(active);
            }
        } else if first && is_frontmatter_open(line) {
            closer = Some(BlockCloser::Frontmatter { seen: 0 });
        } else {
            let cut = chars >= target_chars
                && (is_atx_heading(line)
                    || (after_blank_line && is_block_start(line) && !is_ordered_item(line)));
            if cut {
                flush(&mut chunks, &mut lines, &mut chars);
            }
            closer = block_closer(line);
            let definition = (first || after_blank_line || after_definition || after_heading)
                && is_link_definition(line);
            after_definition = definition;
            let line_len = line.encode_utf16().count();
            if definition && definition_chars + line_len < SHARED_DEFINITIONS_MAX_CHARS {
                definitions.push(line.trim());
                definition_chars += line_len + 1;
            }
        }
        first = false;
        let line_len = line.encode_utf16().count();
        lines.push(line);
        chars += line_len + 1;
        after_blank_line = line.trim().is_empty();
        after_heading = closer.is_none() && (is_atx_heading(line) || is_thematic_break(line));
    }
    flush(&mut chunks, &mut lines, &mut chars);
    SplitMarkdown {
        chunks,
        definitions: definitions.join("\n"),
    }
}

/// Appends shared link definitions after `text` when non-empty (`withMarkdownDefinitions`).
#[must_use]
pub fn with_markdown_definitions(text: &str, definitions: &str) -> String {
    if definitions.is_empty() {
        text.to_owned()
    } else {
        format!("{text}\n\n{definitions}")
    }
}

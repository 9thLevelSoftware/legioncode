#![forbid(unsafe_code)]
//! Lightweight TypeScript/JavaScript lexer for erasable TS stripping and import scanning.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokKind {
    Word,
    Number,
    String,
    TemplateNoSub,
    TemplateHead,
    TemplateMiddle,
    TemplateTail,
    Regex,
    Punct,
    LineComment,
    BlockComment,
    Newline,
    Whitespace,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Tok {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
    pub opens_control: bool,
}

impl Tok {
    #[inline]
    pub fn text(self, src: &str) -> &str {
        &src[self.start..self.end]
    }

    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(
            self.kind,
            TokKind::Whitespace | TokKind::Newline | TokKind::LineComment | TokKind::BlockComment
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ParenEntry {
    control: bool,
}

struct LexState {
    toks: Vec<Tok>,
    i: usize,
    last_sig: Option<usize>,
    tpl_braces: Vec<usize>,
    brace_depth: usize,
    parens: Vec<ParenEntry>,
}

pub(crate) fn lex_ts(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut state = LexState {
        toks: Vec::new(),
        i: 0,
        last_sig: None,
        tpl_braces: Vec::new(),
        brace_depth: 0,
        parens: Vec::new(),
    };
    while state.i < b.len() {
        if !lex_trivia_or_literal(src, b, &mut state) {
            lex_ident_or_punct(src, b, &mut state);
        }
    }
    state.toks
}

fn lex_trivia_or_literal(src: &str, b: &[u8], s: &mut LexState) -> bool {
    let i = s.i;
    let len = b.len();
    let ch = b[i];
    if ch == b'\n' || ch == b'\r' {
        let next = if ch == b'\r' && i + 1 < len && b[i + 1] == b'\n' {
            i + 2
        } else {
            i + 1
        };
        s.toks.push(tok(TokKind::Newline, i, next, false));
        s.i = next;
        return true;
    }
    if ch == b' ' || ch == b'\t' || ch == 0x0b || ch == 0x0c {
        let mut end = i;
        while end < len && matches!(b[end], b' ' | b'\t' | 0x0b | 0x0c) {
            end += 1;
        }
        s.toks.push(tok(TokKind::Whitespace, i, end, false));
        s.i = end;
        return true;
    }
    if ch == b'/' && i + 1 < len && b[i + 1] == b'/' {
        let mut end = i + 2;
        while end < len && b[end] != b'\n' && b[end] != b'\r' {
            end += 1;
        }
        s.toks.push(tok(TokKind::LineComment, i, end, false));
        s.i = end;
        return true;
    }
    if ch == b'/' && i + 1 < len && b[i + 1] == b'*' {
        let mut end = i + 2;
        while end + 1 < len && !(b[end] == b'*' && b[end + 1] == b'/') {
            end += 1;
        }
        let next = (end + 2).min(len);
        s.toks.push(tok(TokKind::BlockComment, i, next, false));
        s.i = next;
        return true;
    }
    if ch == b'"' || ch == b'\'' {
        let next = scan_quoted(b, i, ch);
        push_sig(
            &mut s.toks,
            &mut s.last_sig,
            tok(TokKind::String, i, next, false),
        );
        s.i = next;
        return true;
    }
    if ch == b'`' || (ch == b'}' && s.tpl_braces.last().copied() == Some(s.brace_depth)) {
        let is_close = ch == b'}';
        if is_close {
            s.tpl_braces.pop();
        }
        let (next, has_sub) = scan_template(b, i + 1);
        if has_sub {
            s.tpl_braces.push(s.brace_depth);
        }
        let kind = match (is_close, has_sub) {
            (false, true) => TokKind::TemplateHead,
            (false, false) => TokKind::TemplateNoSub,
            (true, true) => TokKind::TemplateMiddle,
            (true, false) => TokKind::TemplateTail,
        };
        push_sig(&mut s.toks, &mut s.last_sig, tok(kind, i, next, false));
        s.i = next;
        return true;
    }
    if ch == b'/' && regex_allowed(src, &s.toks, s.last_sig) {
        let next = scan_regex(b, i + 1);
        push_sig(
            &mut s.toks,
            &mut s.last_sig,
            tok(TokKind::Regex, i, next, false),
        );
        s.i = next;
        return true;
    }
    false
}

fn lex_ident_or_punct(src: &str, b: &[u8], s: &mut LexState) {
    let i = s.i;
    let len = b.len();
    let ch = b[i];
    if ch.is_ascii_digit() || (ch == b'.' && i + 1 < len && b[i + 1].is_ascii_digit()) {
        let next = scan_number(b, i);
        push_sig(
            &mut s.toks,
            &mut s.last_sig,
            tok(TokKind::Number, i, next, false),
        );
        s.i = next;
        return;
    }
    if is_ident_start(src, i) {
        let next = scan_ident(src, i);
        push_sig(
            &mut s.toks,
            &mut s.last_sig,
            tok(TokKind::Word, i, next, false),
        );
        s.i = next;
        return;
    }
    let mut opens_control = false;
    let next = if ch == b'{' {
        s.brace_depth += 1;
        i + 1
    } else if ch == b'}' {
        s.brace_depth = s.brace_depth.saturating_sub(1);
        i + 1
    } else if ch == b'(' {
        let control = s
            .last_sig
            .is_some_and(|idx| matches!(s.toks[idx].text(src), "if" | "while" | "for" | "with"));
        s.parens.push(ParenEntry { control });
        i + 1
    } else if ch == b')' {
        opens_control = s.parens.pop().is_some_and(|p| p.control);
        i + 1
    } else {
        scan_punct(b, i)
    };
    push_sig(
        &mut s.toks,
        &mut s.last_sig,
        tok(TokKind::Punct, i, next, opens_control),
    );
    s.i = next;
}

fn tok(kind: TokKind, start: usize, end: usize, opens_control: bool) -> Tok {
    Tok {
        kind,
        start,
        end,
        opens_control,
    }
}

fn push_sig(toks: &mut Vec<Tok>, last_sig: &mut Option<usize>, t: Tok) {
    *last_sig = Some(toks.len());
    toks.push(t);
}

fn regex_allowed(src: &str, toks: &[Tok], last_sig: Option<usize>) -> bool {
    let Some(idx) = last_sig else {
        return true;
    };
    let prev = toks[idx];
    match prev.kind {
        TokKind::Number
        | TokKind::String
        | TokKind::TemplateNoSub
        | TokKind::TemplateTail
        | TokKind::Regex => false,
        TokKind::TemplateHead
        | TokKind::TemplateMiddle
        | TokKind::Whitespace
        | TokKind::Newline
        | TokKind::LineComment
        | TokKind::BlockComment => true,
        TokKind::Word => matches!(
            prev.text(src),
            "return"
                | "throw"
                | "case"
                | "default"
                | "yield"
                | "await"
                | "typeof"
                | "void"
                | "delete"
                | "in"
                | "instanceof"
                | "of"
                | "else"
                | "do"
                | "new"
                | "export"
        ),
        TokKind::Punct => match prev.text(src) {
            ")" => prev.opens_control,
            "]" | "++" | "--" => false,
            _ => true,
        },
    }
}

fn scan_quoted(b: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        if b[i] == b'\\' {
            i = (i + 2).min(b.len());
            continue;
        }
        if b[i] == quote {
            return i + 1;
        }
        if b[i] == b'\n' || b[i] == b'\r' {
            return i;
        }
        i += 1;
    }
    b.len()
}

fn scan_template(b: &[u8], mut i: usize) -> (usize, bool) {
    while i < b.len() {
        if b[i] == b'\\' {
            i = (i + 2).min(b.len());
            continue;
        }
        if b[i] == b'`' {
            return (i + 1, false);
        }
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'{' {
            return (i + 2, true);
        }
        i += 1;
    }
    (b.len(), false)
}

fn scan_regex(b: &[u8], mut i: usize) -> usize {
    let mut in_class = false;
    while i < b.len() {
        if b[i] == b'\\' {
            i = (i + 2).min(b.len());
            continue;
        }
        if b[i] == b'[' {
            in_class = true;
        } else if b[i] == b']' && in_class {
            in_class = false;
        } else if b[i] == b'/' && !in_class {
            i += 1;
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            return i;
        } else if b[i] == b'\n' || b[i] == b'\r' {
            return i;
        }
        i += 1;
    }
    b.len()
}

fn scan_number(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphanumeric()
            || c == b'_'
            || c == b'.'
            || ((c == b'+' || c == b'-') && i > 0 && matches!(b[i - 1], b'e' | b'E'))
        {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn is_ident_start(src: &str, idx: usize) -> bool {
    let Some(ch) = src[idx..].chars().next() else {
        return false;
    };
    ch == '_' || ch == '$' || ch.is_alphabetic()
}

fn scan_ident(src: &str, mut idx: usize) -> usize {
    while let Some(ch) = src[idx..].chars().next() {
        if ch == '_' || ch == '$' || ch.is_alphanumeric() {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    idx
}

fn scan_punct(b: &[u8], i: usize) -> usize {
    const OPS: &[&[u8]] = &[
        b">>>=", b">>>", b"===", b"!==", b"...", b"<<=", b">>=", b"**=", b"&&=", b"||=", b"??=",
        b"=>", b"?.", b"??", b"&&", b"||", b"==", b"!=", b"<=", b">=", b"++", b"--", b"<<", b">>",
        b"**", b"+=", b"-=", b"*=", b"/=", b"%=", b"&=", b"|=", b"^=",
    ];
    let rem = &b[i..];
    for pat in OPS {
        if rem.starts_with(pat) {
            return i + pat.len();
        }
    }
    i + 1
}

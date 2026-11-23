#![forbid(unsafe_code)]
//! Type expression boundary scanning, non-null postfix, and modifier TS stripping helpers.

use crate::ts_lexer::TokKind;
use crate::ts_strip::StripState;

impl StripState<'_> {
    pub(crate) fn scan_type_expr_end(&self, start: usize, stop_at_brace_or_arrow: bool) -> usize {
        let n = self.sig.len();
        let mut i = start;
        let mut paren = 0usize;
        let mut bracket = 0usize;
        let mut brace = 0usize;
        let mut angle = 0usize;
        while i < n {
            let t = self.text(i);
            let at_top = paren == 0 && bracket == 0 && brace == 0 && angle == 0;
            if at_top
                && let Some(end_idx) =
                    self.check_type_stop_at_top(start, i, t, stop_at_brace_or_arrow)
            {
                if end_idx == usize::MAX {
                    if let Some(c) = self.match_pair(i, "{", "}") {
                        i = c + 1;
                        continue;
                    }
                } else {
                    return end_idx;
                }
            }
            match t {
                "(" => paren += 1,
                ")" => paren = paren.saturating_sub(1),
                "[" => bracket += 1,
                "]" => bracket = bracket.saturating_sub(1),
                "{" => brace += 1,
                "}" => brace = brace.saturating_sub(1),
                "<" => angle += 1,
                ">" => angle = angle.saturating_sub(1),
                ">>" => angle = angle.saturating_sub(2),
                _ => {}
            }
            if paren == 0
                && bracket == 0
                && brace == 0
                && angle == 0
                && i + 1 < n
                && self.has_newline_between(i, i + 1)
                && !matches!(
                    t,
                    "|" | "&"
                        | "=>"
                        | ":"
                        | "extends"
                        | "infer"
                        | "keyof"
                        | "typeof"
                        | "readonly"
                        | "."
                        | "<"
                )
                && !matches!(self.text(i + 1), "|" | "&" | "." | "[" | "extends")
            {
                return i + 1;
            }
            i += 1;
        }
        n
    }

    fn check_type_stop_at_top(
        &self,
        start: usize,
        i: usize,
        t: &str,
        stop_at_brace_or_arrow: bool,
    ) -> Option<usize> {
        let n = self.sig.len();
        if matches!(t, ";" | "," | ")" | "]" | "}" | "=" | "?") {
            return Some(i);
        }
        if stop_at_brace_or_arrow
            && t == "=>"
            && ((i > start && self.text(i - 1) == ")") || i + 1 < n)
        {
            return Some(i);
        }
        if stop_at_brace_or_arrow && t == "{" {
            let is_object_type_pos =
                i == start || matches!(self.text(i - 1), "|" | "&" | "=>" | "readonly");
            if !is_object_type_pos {
                return Some(i);
            }
            if let Some(c) = self.match_pair(i, "{", "}") {
                let next = c + 1;
                let cont = next < n
                    && matches!(
                        self.text(next),
                        "{" | "=>" | "|" | "&" | "[" | "is" | "asserts"
                    );
                return Some(if cont { usize::MAX } else { i });
            }
        }
        if matches!(
            t,
            "&&" | "||"
                | "??"
                | "==="
                | "!=="
                | "=="
                | "!="
                | "<="
                | ">="
                | "+"
                | "-"
                | "*"
                | "/"
                | "%"
                | "^"
                | "<<"
                | ">>"
                | ">>>"
                | "in"
                | "instanceof"
                | "of"
                | "as"
                | "satisfies"
        ) {
            return Some(i);
        }
        if i > start && t == "{" && !matches!(self.text(i - 1), "|" | "&" | "=>" | "extends") {
            return Some(i);
        }
        None
    }

    pub(crate) fn is_expr_end(&self, k: usize) -> bool {
        let t = self.text(k);
        matches!(t, ")" | "]" | "}" | "!")
            || matches!(
                self.kind(k),
                TokKind::Number
                    | TokKind::String
                    | TokKind::TemplateNoSub
                    | TokKind::TemplateTail
                    | TokKind::Regex
            )
            || (self.kind(k) == TokKind::Word
                && !matches!(
                    t,
                    "return"
                        | "throw"
                        | "yield"
                        | "await"
                        | "typeof"
                        | "void"
                        | "delete"
                        | "in"
                        | "instanceof"
                        | "of"
                        | "new"
                        | "else"
                        | "case"
                        | "default"
                        | "export"
                        | "import"
                ))
    }

    pub(crate) fn is_postfix_non_null(&self, k: usize) -> bool {
        let prev = self.toks[self.sig[k - 1]];
        if prev.opens_control {
            return false;
        }
        let ptxt = self.text(k - 1);
        let prev_ok = matches!(ptxt, ")" | "]")
            || matches!(
                self.kind(k - 1),
                TokKind::Word | TokKind::String | TokKind::TemplateNoSub | TokKind::TemplateTail
            );
        if !prev_ok {
            return false;
        }
        if k + 1 >= self.sig.len() {
            return true;
        }
        let next = self.text(k + 1);
        if matches!(
            self.kind(k + 1),
            TokKind::Number
                | TokKind::String
                | TokKind::TemplateNoSub
                | TokKind::TemplateHead
                | TokKind::Regex
        ) || matches!(next, "{" | "!" | "~")
        {
            return false;
        }
        self.kind(k + 1) != TokKind::Word
            || matches!(next, "as" | "satisfies" | "in" | "instanceof")
    }

    pub(crate) fn is_class_member_modifier(&self, k: usize) -> bool {
        if k + 1 >= self.sig.len() {
            return false;
        }
        let prev_ok = k == 0
            || matches!(self.text(k - 1), "{" | ";" | "}" | "(" | ",")
            || self.has_newline_between(k - 1, k)
            || matches!(
                self.text(k - 1),
                "public" | "private" | "protected" | "readonly" | "override" | "abstract"
            );
        if !prev_ok {
            return false;
        }
        let next = self.text(k + 1);
        (self.kind(k + 1) == TokKind::Word && !matches!(next, "from" | "as" | "in" | "of"))
            || matches!(next, "[" | "*")
    }
}

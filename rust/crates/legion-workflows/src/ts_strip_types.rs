#![forbid(unsafe_code)]
//! Type-argument, parameter-list, return-type, and assertion TS stripping helpers.

use crate::ts_lexer::TokKind;
use crate::ts_strip::StripState;

impl StripState<'_> {
    pub(crate) fn match_angle_balanced(&self, open: usize) -> Option<usize> {
        let n = self.sig.len();
        let mut angle = 0usize;
        let mut paren = 0usize;
        let mut brace = 0usize;
        let mut bracket = 0usize;
        let mut i = open;
        while i < n {
            let t = self.text(i);
            match t {
                "(" => paren += 1,
                ")" => {
                    if paren == 0 {
                        return None;
                    }
                    paren -= 1;
                }
                "{" => brace += 1,
                "}" => {
                    if brace == 0 {
                        return None;
                    }
                    brace -= 1;
                }
                "[" => bracket += 1,
                "]" => {
                    if bracket == 0 {
                        return None;
                    }
                    bracket -= 1;
                }
                "<" if paren == 0 && brace == 0 && bracket == 0 => angle += 1,
                ">" if paren == 0 && brace == 0 && bracket == 0 => {
                    angle -= 1;
                    if angle == 0 {
                        return Some(i);
                    }
                }
                ">>" if paren == 0 && brace == 0 && bracket == 0 => {
                    if angle <= 2 {
                        return (angle == 2).then_some(i);
                    }
                    angle -= 2;
                }
                ";" => return None,
                _ => {}
            }
            i += 1;
        }
        None
    }

    pub(crate) fn is_arrow_generic_start(&self, k: usize) -> bool {
        let n = self.sig.len();
        if k + 2 >= n {
            return false;
        }
        let allowed_prev = k == 0
            || matches!(
                self.text(k - 1),
                "=" | "("
                    | "["
                    | ","
                    | ":"
                    | "?"
                    | "{"
                    | ";"
                    | "return"
                    | "=>"
                    | "throw"
                    | "await"
                    | "default"
                    | "export"
            );
        allowed_prev
            && (self.kind(k + 1) == TokKind::Word
                || matches!(self.text(k + 1), "const" | "in" | "out"))
    }

    pub(crate) fn match_angle_before_paren(&self, k: usize) -> Option<usize> {
        let close = self.match_angle_balanced(k)?;
        let next = close + 1;
        if next >= self.sig.len() || self.text(next) != "(" {
            return None;
        }
        let has_comma_or_extends =
            (k + 1..close).any(|i| matches!(self.text(i), "," | "extends" | "="));
        let single_word = close == k + 2 && self.kind(k + 1) == TokKind::Word;
        (has_comma_or_extends || single_word).then_some(close)
    }

    pub(crate) fn is_call_generic_start(&self, k: usize) -> bool {
        if k == 0 || k + 2 >= self.sig.len() {
            return false;
        }
        let prev = self.text(k - 1);
        let prev_is_expr = matches!(prev, ")" | "]")
            || (self.kind(k - 1) == TokKind::Word
                && !matches!(
                    prev,
                    "if" | "while"
                        | "for"
                        | "return"
                        | "throw"
                        | "yield"
                        | "await"
                        | "typeof"
                        | "void"
                        | "delete"
                        | "in"
                        | "instanceof"
                        | "of"
                        | "case"
                        | "else"
                        | "new"
                        | "export"
                        | "default"
                ));
        prev_is_expr
            && (self.kind(k + 1) == TokKind::Word
                || matches!(
                    self.text(k + 1),
                    "{" | "[" | "(" | "readonly" | "keyof" | "typeof"
                )
                || matches!(self.kind(k + 1), TokKind::String | TokKind::Number))
    }

    pub(crate) fn match_angle_call_args(&self, k: usize) -> Option<usize> {
        let close = self.match_angle_balanced(k)?;
        let next = close + 1;
        if next >= self.sig.len() {
            return None;
        }
        let nt = self.text(next);
        if nt != "("
            && !matches!(
                self.kind(next),
                TokKind::TemplateNoSub | TokKind::TemplateHead
            )
        {
            return None;
        }
        let looks_type = (k + 1..close).all(|i| {
            !matches!(
                self.text(i),
                "&&" | "||" | "??" | "===" | "!==" | "==" | "!=" | ";"
            )
        });
        looks_type.then_some(close)
    }

    pub(crate) fn is_param_list_paren(&self, open: usize, close: usize) -> bool {
        let n = self.sig.len();
        if close + 1 >= n {
            return false;
        }
        if self.text(close + 1) == "=>" {
            return true;
        }
        if self.text(close + 1) == ":" && self.is_arrow_return_type(close + 1) {
            return true;
        }
        if open > 0 && self.kind(open - 1) == TokKind::Word {
            let fn_name = self.text(open - 1);
            if !matches!(
                fn_name,
                "if" | "while"
                    | "for"
                    | "switch"
                    | "catch"
                    | "with"
                    | "return"
                    | "throw"
                    | "typeof"
                    | "void"
                    | "delete"
                    | "await"
                    | "yield"
            ) {
                if self.text(close + 1) == "{" {
                    return open >= 2 || self.has_param_annotation(open, close);
                }
                if self.text(close + 1) == ":" {
                    let after_ret = self.scan_type_expr_end(close + 2, true);
                    return after_ret < n && self.text(after_ret) == "{";
                }
            }
        }
        false
    }

    fn has_param_annotation(&self, open: usize, close: usize) -> bool {
        let mut depth = 0usize;
        let mut i = open + 1;
        while i < close {
            let t = self.text(i);
            if matches!(t, "(" | "[" | "{") {
                depth += 1;
            } else if matches!(t, ")" | "]" | "}") {
                depth = depth.saturating_sub(1);
            } else if depth == 0
                && (t == ":" || (t == "?" && i + 1 < close && self.text(i + 1) == ":"))
            {
                return true;
            }
            i += 1;
        }
        false
    }

    pub(crate) fn strip_param_list(&mut self, open: usize, close: usize) {
        let mut i = open + 1;
        while i < close {
            if self.text(i) == "..." {
                i += 1;
            }
            if i < close && self.text(i) == "this" && i + 1 < close && self.text(i + 1) == ":" {
                let mut end = i + 1;
                let mut depth = 0usize;
                while end < close {
                    let t = self.text(end);
                    if matches!(t, "(" | "[" | "{" | "<") {
                        depth += 1;
                    } else if matches!(t, ")" | "]" | "}" | ">") {
                        depth = depth.saturating_sub(1);
                    } else if t == "," && depth == 0 {
                        end += 1;
                        break;
                    }
                    end += 1;
                }
                self.mark_sig_span(i, end);
                i = end;
                continue;
            }
            while i < close
                && matches!(
                    self.text(i),
                    "public" | "private" | "protected" | "readonly" | "override"
                )
            {
                self.mark_sig_span(i, i + 1);
                i += 1;
            }
            if i >= close {
                break;
            }
            if self.text(i) == "{" {
                if let Some(c) = self.match_pair(i, "{", "}") {
                    i = (c + 1).min(close);
                } else {
                    break;
                }
            } else if self.text(i) == "[" {
                if let Some(c) = self.match_pair(i, "[", "]") {
                    i = (c + 1).min(close);
                } else {
                    break;
                }
            } else if self.kind(i) == TokKind::Word {
                i += 1;
            } else {
                i += 1;
                continue;
            }
            if i < close && self.text(i) == "?" {
                self.mark_sig_span(i, i + 1);
                i += 1;
            }
            if i < close && self.text(i) == ":" {
                let end = self.scan_param_type_end(i + 1, close);
                self.mark_sig_span(i, end);
                i = end;
            }
            let mut depth = 0usize;
            while i < close {
                let t = self.text(i);
                if matches!(t, "(" | "[" | "{") {
                    depth += 1;
                } else if matches!(t, ")" | "]" | "}") {
                    depth = depth.saturating_sub(1);
                } else if t == "," && depth == 0 {
                    i += 1;
                    break;
                }
                i += 1;
            }
        }
    }

    fn scan_param_type_end(&self, start: usize, close: usize) -> usize {
        let mut i = start;
        let mut paren = 0usize;
        let mut bracket = 0usize;
        let mut brace = 0usize;
        let mut angle = 0usize;
        while i < close {
            let t = self.text(i);
            if paren == 0 && bracket == 0 && brace == 0 && angle == 0 && matches!(t, "," | "=") {
                return i;
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
                _ => {}
            }
            i += 1;
        }
        close
    }

    pub(crate) fn handle_post_param_return_type(&mut self, after_paren: usize) -> usize {
        let n = self.sig.len();
        if after_paren < n && self.text(after_paren) == ":" {
            let end = self.scan_type_expr_end(after_paren + 1, true);
            if end < n && matches!(self.text(end), "{" | "=>") {
                self.mark_sig_span(after_paren, end);
                return end;
            }
        }
        after_paren
    }

    pub(crate) fn is_optional_param_marker(&self, k: usize) -> bool {
        k + 1 < self.sig.len() && self.text(k + 1) == ":"
    }

    pub(crate) fn is_arrow_return_type(&self, colon: usize) -> bool {
        let n = self.sig.len();
        if colon == 0 || self.text(colon - 1) != ")" {
            return false;
        }
        let end = self.scan_type_expr_end(colon + 1, true);
        end < n && self.text(end) == "=>"
    }
}

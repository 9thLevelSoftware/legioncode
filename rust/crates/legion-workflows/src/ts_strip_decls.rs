#![forbid(unsafe_code)]
//! Class, variable, and function header TS stripping helpers for `ts_strip`.

use crate::ts_lexer::TokKind;
use crate::ts_strip::StripState;

impl StripState<'_> {
    pub(crate) fn handle_class_header(&mut self, k: usize) -> usize {
        let n = self.sig.len();
        let mut i = k + 1;
        if i < n
            && self.kind(i) == TokKind::Word
            && self.text(i) != "extends"
            && self.text(i) != "implements"
        {
            i += 1;
        }
        if i < n
            && self.text(i) == "<"
            && let Some(close) = self.match_angle_balanced(i)
        {
            self.mark_sig_span(i, close + 1);
            i = close + 1;
        }
        while i < n && self.text(i) != "{" {
            if self.text(i) == "implements" {
                let mut end = i + 1;
                while end < n && self.text(end) != "{" {
                    end += 1;
                }
                self.mark_sig_span(i, end);
                return end + 1;
            }
            if self.text(i) == "<"
                && let Some(close) = self.match_angle_balanced(i)
            {
                self.mark_sig_span(i, close + 1);
                i = close + 1;
                continue;
            }
            i += 1;
        }
        (i + 1).min(n)
    }

    pub(crate) fn at_var_decl(&self, k: usize) -> bool {
        if k == 0 {
            return true;
        }
        let prev = self.text(k - 1);
        if matches!(prev, ";" | "{" | "}" | "export" | "(" | "else") {
            return true;
        }
        self.at_stmt_start(k)
    }

    pub(crate) fn handle_var_decl(&mut self, k: usize) -> usize {
        let n = self.sig.len();
        let mut i = k + 1;
        loop {
            if i >= n {
                return n;
            }
            if self.text(i) == "{" {
                let Some(close) = self.match_pair(i, "{", "}") else {
                    return i + 1;
                };
                i = close + 1;
            } else if self.text(i) == "[" {
                let Some(close) = self.match_pair(i, "[", "]") else {
                    return i + 1;
                };
                i = close + 1;
            } else if self.kind(i) == TokKind::Word {
                i += 1;
            } else {
                return i;
            }
            if i < n && self.text(i) == "!" {
                self.mark_sig_span(i, i + 1);
                i += 1;
            }
            if i < n && self.text(i) == ":" {
                let end = self.scan_type_expr_end(i + 1, false);
                self.mark_sig_span(i, end);
                i = end;
            }
            if i < n && self.text(i) == "," {
                i += 1;
                continue;
            }
            return i;
        }
    }

    pub(crate) fn handle_function_header(&mut self, k: usize) -> usize {
        let n = self.sig.len();
        let mut i = k + 1;
        if i < n && self.text(i) == "*" {
            i += 1;
        }
        if i < n && self.kind(i) == TokKind::Word {
            i += 1;
        }
        if i < n
            && self.text(i) == "<"
            && let Some(close) = self.match_angle_balanced(i)
        {
            self.mark_sig_span(i, close + 1);
            i = close + 1;
        }
        if i < n
            && self.text(i) == "("
            && let Some(close) = self.match_pair(i, "(", ")")
        {
            self.strip_param_list(i, close);
            return self.handle_post_param_return_type(close + 1);
        }
        i
    }
}

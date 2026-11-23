#![forbid(unsafe_code)]
//! Import/export/declaration TypeScript stripping helpers for `ts_strip`.

use crate::ts_lexer::TokKind;
use crate::ts_strip::{ExternalImport, ExternalImportKind, StripState};

impl StripState<'_> {
    pub(crate) fn record_specifier(&mut self, raw: &str, line: usize, kind: ExternalImportKind) {
        let spec = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(raw)
            .to_owned();
        if spec.starts_with("./") || spec.starts_with("../") || spec.starts_with('/') {
            self.relative_imports.push(spec);
        } else {
            self.external_imports.push(ExternalImport {
                specifier: spec,
                line,
                kind,
            });
        }
    }

    pub(crate) fn handle_import(&mut self, k: usize) -> usize {
        let n = self.sig.len();
        if k + 1 < n && self.text(k + 1) == "(" {
            return k + 1;
        }
        if k + 1 < n && self.kind(k + 1) == TokKind::String {
            self.record_specifier(
                self.text(k + 1),
                self.line_of(k),
                ExternalImportKind::Static,
            );
            return if k + 2 < n && self.text(k + 2) == ";" {
                k + 3
            } else {
                k + 2
            };
        }
        if k + 2 < n
            && self.text(k + 1) == "type"
            && (matches!(self.text(k + 2), "{" | "*")
                || (self.kind(k + 2) == TokKind::Word && !matches!(self.text(k + 2), "from" | ",")))
        {
            let end = self.import_export_stmt_end(k);
            self.mark_sig_span(k, end);
            return end;
        }
        let end = self.import_export_stmt_end(k);
        let from_idx = self.find_from_string(k + 1, end);
        let brace_open = (k + 1..end).find(|&i| self.text(i) == "{");
        let mut all_dropped = false;
        if let Some(open) = brace_open
            && let Some(close) = self.match_pair(open, "{", "}")
            && close < end
        {
            let keep_count = self.strip_type_specifiers_in_braces(open, close);
            let has_default = open > k + 1;
            if keep_count == 0 && !has_default {
                self.mark_sig_span(k, end);
                all_dropped = true;
            } else if keep_count == 0 && has_default && self.text(open - 1) == "," {
                self.mark_sig_span(open - 1, close + 1);
            }
        }
        if !all_dropped && let Some(spec_idx) = from_idx {
            self.record_specifier(
                self.text(spec_idx),
                self.line_of(spec_idx),
                ExternalImportKind::Static,
            );
        }
        end
    }

    fn import_export_stmt_end(&self, mut i: usize) -> usize {
        let n = self.sig.len();
        let mut depth = 0usize;
        while i < n {
            let t = self.text(i);
            if t == "{" {
                depth += 1;
            } else if t == "}" {
                depth = depth.saturating_sub(1);
            } else if t == ";" && depth == 0 {
                return i + 1;
            } else if depth == 0 && t == "from" && i + 1 < n && self.kind(i + 1) == TokKind::String
            {
                return if i + 2 < n && self.text(i + 2) == ";" {
                    i + 3
                } else {
                    i + 2
                };
            } else if depth == 0
                && i + 1 < n
                && self.has_newline_between(i, i + 1)
                && !matches!(t, "import" | "export" | "type" | "from" | "," | "as" | "*")
                && !matches!(self.text(i + 1), "from" | ";")
            {
                return i + 1;
            }
            i += 1;
        }
        n
    }

    fn find_from_string(&self, from: usize, to: usize) -> Option<usize> {
        let mut i = from;
        while i + 1 < to {
            if self.text(i) == "from" && self.kind(i + 1) == TokKind::String {
                return Some(i + 1);
            }
            i += 1;
        }
        None
    }

    fn strip_type_specifiers_in_braces(&mut self, open: usize, close: usize) -> usize {
        let mut items: Vec<(usize, usize, bool)> = Vec::new();
        let mut i = open + 1;
        while i < close {
            let start = i;
            let mut comma = i;
            while comma < close && self.text(comma) != "," {
                comma += 1;
            }
            let is_type = start + 1 < comma
                && self.text(start) == "type"
                && self.kind(start + 1) == TokKind::Word
                && self.text(start + 1) != "as";
            items.push((start, comma, is_type));
            i = comma + 1;
        }
        let keep_count = items.iter().filter(|x| !x.2).count();
        if keep_count == 0 {
            return 0;
        }
        let mut kept_any_before = false;
        for (idx, &(start, comma, is_type)) in items.iter().enumerate() {
            if is_type {
                if comma < close {
                    self.mark_sig_span(start, comma + 1);
                } else if kept_any_before && start > open + 1 && self.text(start - 1) == "," {
                    self.mark_sig_span(start - 1, comma);
                } else {
                    self.mark_sig_span(start, comma);
                }
            } else {
                kept_any_before = true;
                let remaining_kept = items[idx + 1..].iter().any(|x| !x.2);
                if !remaining_kept && comma < close {
                    self.mark_sig_span(comma, comma + 1);
                }
            }
        }
        keep_count
    }

    pub(crate) fn handle_export(&mut self, k: usize) -> Option<usize> {
        let n = self.sig.len();
        if k + 1 >= n {
            return None;
        }
        let next = self.text(k + 1);
        if next == "type" && k + 2 < n {
            if matches!(self.text(k + 2), "{" | "*") {
                let end = self.import_export_stmt_end(k);
                self.mark_sig_span(k, end);
                return Some(end);
            }
            if self.is_type_alias_start(k + 1) {
                return Some(self.strip_type_alias(k, k + 1));
            }
        }
        if next == "interface" && k + 2 < n && self.kind(k + 2) == TokKind::Word {
            return Some(self.strip_interface(k, k + 1));
        }
        if next == "declare" && self.is_declare_start(k + 1) {
            self.mark_sig_span(k, k + 1);
            return Some(self.strip_declare(k + 1));
        }
        if next == "abstract" && k + 2 < n && self.text(k + 2) == "class" {
            self.mark_sig_span(k + 1, k + 2);
            return Some(k + 2);
        }
        if next == "{" {
            let end = self.import_export_stmt_end(k);
            let close = self.match_pair(k + 1, "{", "}")?;
            let keep = self.strip_type_specifiers_in_braces(k + 1, close);
            let from_idx = self.find_from_string(close + 1, end);
            if keep == 0 && from_idx.is_some() {
                self.mark_sig_span(k, end);
            } else if let Some(spec_idx) = from_idx {
                self.record_specifier(
                    self.text(spec_idx),
                    self.line_of(spec_idx),
                    ExternalImportKind::Static,
                );
            }
            return Some(end);
        }
        if next == "*" {
            let end = self.import_export_stmt_end(k);
            if let Some(spec_idx) = self.find_from_string(k + 2, end) {
                self.record_specifier(
                    self.text(spec_idx),
                    self.line_of(spec_idx),
                    ExternalImportKind::Static,
                );
            }
            return Some(end);
        }
        None
    }

    pub(crate) fn strip_interface(&mut self, strip_from: usize, kw: usize) -> usize {
        let n = self.sig.len();
        let mut i = kw + 1;
        while i < n && self.text(i) != "{" {
            i += 1;
        }
        if let Some(close) = (i < n).then(|| self.match_pair(i, "{", "}")).flatten() {
            self.mark_sig_span(strip_from, close + 1);
            return close + 1;
        }
        self.mark_sig_span(strip_from, n);
        n
    }

    pub(crate) fn is_type_alias_start(&self, k: usize) -> bool {
        let n = self.sig.len();
        if k + 2 >= n || self.kind(k + 1) != TokKind::Word {
            return false;
        }
        let mut i = k + 2;
        if i < n
            && self.text(i) == "<"
            && let Some(close) = self.match_angle_balanced(i)
        {
            i = close + 1;
        }
        i < n && self.text(i) == "="
    }

    pub(crate) fn strip_type_alias(&mut self, strip_from: usize, kw: usize) -> usize {
        let n = self.sig.len();
        let mut i = kw + 2;
        if i < n
            && self.text(i) == "<"
            && let Some(close) = self.match_angle_balanced(i)
        {
            i = close + 1;
        }
        if i < n && self.text(i) == "=" {
            let end = self.scan_type_expr_end(i + 1, false);
            let final_end = if end < n && self.text(end) == ";" {
                end + 1
            } else {
                end
            };
            self.mark_sig_span(strip_from, final_end);
            return final_end;
        }
        kw + 1
    }

    pub(crate) fn is_declare_start(&self, k: usize) -> bool {
        k + 1 < self.sig.len()
            && matches!(
                self.text(k + 1),
                "const"
                    | "let"
                    | "var"
                    | "function"
                    | "class"
                    | "abstract"
                    | "async"
                    | "enum"
                    | "namespace"
                    | "module"
                    | "type"
                    | "interface"
                    | "global"
            )
    }

    pub(crate) fn strip_declare(&mut self, k: usize) -> usize {
        let n = self.sig.len();
        let mut i = k + 1;
        while i < n {
            let t = self.text(i);
            if t == "{" {
                if let Some(close) = self.match_pair(i, "{", "}") {
                    self.mark_sig_span(k, close + 1);
                    return close + 1;
                }
                break;
            }
            if t == ";" {
                self.mark_sig_span(k, i + 1);
                return i + 1;
            }
            if i + 1 < n
                && self.has_newline_between(i, i + 1)
                && !matches!(t, "=" | ":" | "|" | "&" | "," | "(" | "[")
            {
                self.mark_sig_span(k, i + 1);
                return i + 1;
            }
            i += 1;
        }
        self.mark_sig_span(k, n);
        n
    }
}

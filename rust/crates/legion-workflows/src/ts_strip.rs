#![forbid(unsafe_code)]
//! Erasable TypeScript type-stripping (line-preserving) and external import detection.

use crate::ts_lexer::{Tok, TokKind, lex_ts};

/// Kind of external module import found in workflow source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalImportKind {
    /// Static `import ... from "spec"` or `import "spec"`.
    Static,
    /// Dynamic `import("spec")`.
    Dynamic,
}

/// Non-relative runtime import specifier requiring an external JS runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalImport {
    /// Imported module specifier (e.g. `node:fs` or `zod`).
    pub specifier: String,
    /// 1-based line number of the import.
    pub line: usize,
    /// Static vs dynamic import.
    pub kind: ExternalImportKind,
}

/// Result of stripping TypeScript syntax and scanning runtime imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrippedSource {
    /// Executable JavaScript preserving 1-based line numbers.
    pub code: String,
    /// Non-relative runtime imports found outside type-only imports.
    pub external_imports: Vec<ExternalImport>,
    /// Relative (`./` or `../`) runtime imports found outside type-only imports.
    pub relative_imports: Vec<String>,
}

/// Strips erasable TypeScript annotations while preserving exact line numbers.
pub fn strip_typescript(src: &str) -> StrippedSource {
    let toks = lex_ts(src);
    let sig: Vec<usize> = toks
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.is_trivia())
        .map(|(i, _)| i)
        .collect();
    let mut state = StripState::new(src, &toks, &sig);
    state.run();
    StrippedSource {
        code: state.render(),
        external_imports: state.external_imports,
        relative_imports: state.relative_imports,
    }
}

/// Returns only non-relative runtime imports detected in TypeScript/JavaScript source.
pub fn detect_external_imports(src: &str) -> Vec<ExternalImport> {
    strip_typescript(src).external_imports
}

pub(crate) struct StripState<'a> {
    pub(crate) src: &'a str,
    pub(crate) toks: &'a [Tok],
    pub(crate) sig: &'a [usize],
    pub(crate) drop: Vec<bool>,
    pub(crate) external_imports: Vec<ExternalImport>,
    pub(crate) relative_imports: Vec<String>,
}

impl<'a> StripState<'a> {
    fn new(src: &'a str, toks: &'a [Tok], sig: &'a [usize]) -> Self {
        Self {
            src,
            toks,
            sig,
            drop: vec![false; toks.len()],
            external_imports: Vec::new(),
            relative_imports: Vec::new(),
        }
    }

    pub(crate) fn text(&self, si: usize) -> &'a str {
        self.toks[self.sig[si]].text(self.src)
    }

    pub(crate) fn kind(&self, si: usize) -> TokKind {
        self.toks[self.sig[si]].kind
    }

    pub(crate) fn line_of(&self, si: usize) -> usize {
        let off = self.toks[self.sig[si]].start;
        self.src[..off].bytes().filter(|&b| b == b'\n').count() + 1
    }

    pub(crate) fn has_newline_between(&self, a: usize, b: usize) -> bool {
        let start = self.toks[self.sig[a]].end;
        let end = self.toks[self.sig[b]].start;
        self.src[start..end].contains('\n')
    }

    pub(crate) fn at_stmt_start(&self, k: usize) -> bool {
        if k == 0 {
            return true;
        }
        let prev = self.text(k - 1);
        if matches!(prev, ";" | "{" | "}" | "else" | "do") {
            return true;
        }
        self.has_newline_between(k - 1, k)
            && !matches!(
                prev,
                "." | "?."
                    | ","
                    | ":"
                    | "("
                    | "["
                    | "="
                    | "=>"
                    | "+"
                    | "-"
                    | "*"
                    | "/"
                    | "%"
                    | "&&"
                    | "||"
                    | "??"
            )
    }

    pub(crate) fn mark_sig_span(&mut self, from_si: usize, to_si_excl: usize) {
        if from_si >= to_si_excl || from_si >= self.sig.len() {
            return;
        }
        let start = self.sig[from_si];
        let end = self.sig[to_si_excl.min(self.sig.len()) - 1];
        for idx in start..=end {
            if self.toks[idx].kind != TokKind::Newline {
                self.drop[idx] = true;
            }
        }
    }

    pub(crate) fn match_pair(&self, open_si: usize, open: &str, close: &str) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = open_si;
        while i < self.sig.len() {
            let t = self.text(i);
            if t == open {
                depth += 1;
            } else if t == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            i += 1;
        }
        None
    }

    fn run(&mut self) {
        let n = self.sig.len();
        let mut k = 0usize;
        let mut ternary_stack: Vec<usize> = vec![0];
        let mut in_import_export_braces = false;

        while k < n {
            let txt = self.text(k);
            let knd = self.kind(k);
            if txt == "?" && !self.is_optional_param_marker(k) {
                if let Some(top) = ternary_stack.last_mut() {
                    *top += 1;
                }
                k += 1;
                continue;
            }
            if txt == ":"
                && ternary_stack.last().copied().unwrap_or(0) > 0
                && !self.is_arrow_return_type(k)
            {
                if let Some(top) = ternary_stack.last_mut() {
                    *top -= 1;
                }
                k += 1;
                continue;
            }
            if let Some(next_k) = self.try_strip_stmt_start(k, txt) {
                k = next_k;
                continue;
            }
            if let Some(next_k) = self.try_strip_expr_or_decl(
                k,
                txt,
                knd,
                &mut ternary_stack,
                &mut in_import_export_braces,
            ) {
                k = next_k;
                continue;
            }
            k += 1;
        }
    }

    fn try_strip_stmt_start(&mut self, k: usize, txt: &str) -> Option<usize> {
        let n = self.sig.len();
        if !self.at_stmt_start(k) {
            return None;
        }
        if txt == "import" {
            return Some(self.handle_import(k));
        }
        if txt == "export"
            && let Some(next_k) = self.handle_export(k)
        {
            return Some(next_k);
        }
        if txt == "interface" && k + 1 < n && self.kind(k + 1) == TokKind::Word {
            return Some(self.strip_interface(k, k));
        }
        if txt == "type" && self.is_type_alias_start(k) {
            return Some(self.strip_type_alias(k, k));
        }
        if txt == "declare" && self.is_declare_start(k) {
            return Some(self.strip_declare(k));
        }
        if txt == "abstract" && k + 1 < n && self.text(k + 1) == "class" {
            self.mark_sig_span(k, k + 1);
            return Some(k + 1);
        }
        None
    }

    fn try_strip_expr_or_decl(
        &mut self,
        k: usize,
        txt: &str,
        knd: TokKind,
        ternary_stack: &mut Vec<usize>,
        in_import_export_braces: &mut bool,
    ) -> Option<usize> {
        let n = self.sig.len();
        if txt == "import"
            && k + 2 < n
            && self.text(k + 1) == "("
            && self.kind(k + 2) == TokKind::String
        {
            self.record_specifier(
                self.text(k + 2),
                self.line_of(k),
                ExternalImportKind::Dynamic,
            );
        }
        if txt == "class" {
            ternary_stack.push(0);
            return Some(self.handle_class_header(k));
        }
        if matches!(txt, "const" | "let" | "var") && self.at_var_decl(k) {
            return Some(self.handle_var_decl(k));
        }
        if txt == "function" {
            return Some(self.handle_function_header(k));
        }
        if txt == "<"
            && self.is_arrow_generic_start(k)
            && let Some(close) = self.match_angle_before_paren(k)
        {
            self.mark_sig_span(k, close + 1);
            return Some(close + 1);
        }
        if txt == "<"
            && self.is_call_generic_start(k)
            && let Some(close) = self.match_angle_call_args(k)
        {
            self.mark_sig_span(k, close + 1);
            return Some(close + 1);
        }
        if txt == "(" {
            if let Some(close) = self.match_pair(k, "(", ")")
                && self.is_param_list_paren(k, close)
            {
                self.strip_param_list(k, close);
                return Some(self.handle_post_param_return_type(close + 1));
            }
            ternary_stack.push(0);
            return Some(k + 1);
        }
        if txt == "[" || txt == "{" {
            if txt == "{" && k > 0 && matches!(self.text(k - 1), "import" | "export") {
                *in_import_export_braces = true;
            }
            ternary_stack.push(0);
            return Some(k + 1);
        }
        if txt == ")" || txt == "]" || txt == "}" {
            if txt == "}" {
                *in_import_export_braces = false;
            }
            if ternary_stack.len() > 1 {
                ternary_stack.pop();
            }
            return Some(k + 1);
        }
        if !*in_import_export_braces
            && matches!(txt, "as" | "satisfies")
            && k > 0
            && self.is_expr_end(k - 1)
        {
            let end = self.scan_type_expr_end(k + 1, false);
            self.mark_sig_span(k, end);
            return Some(end);
        }
        if txt == "!" && k > 0 && self.is_postfix_non_null(k) {
            self.mark_sig_span(k, k + 1);
            return Some(k + 1);
        }
        if knd == TokKind::Word
            && matches!(
                txt,
                "public" | "private" | "protected" | "readonly" | "override"
            )
            && self.is_class_member_modifier(k)
        {
            self.mark_sig_span(k, k + 1);
            return Some(k + 1);
        }
        None
    }

    fn render(&self) -> String {
        let mut out = String::with_capacity(self.src.len());
        for (i, t) in self.toks.iter().enumerate() {
            let chunk = t.text(self.src);
            if self.drop[i] {
                for ch in chunk.chars() {
                    out.push(if ch == '\n' || ch == '\r' { ch } else { ' ' });
                }
            } else {
                out.push_str(chunk);
            }
        }
        out
    }
}

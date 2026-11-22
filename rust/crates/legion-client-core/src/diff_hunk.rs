//! Hunk body state machine driving unified diff parsing into [`DiffLineArrays`].

use crate::diff_parser::{DiffChangeType, DiffHunkMeta, ParsedDiffFile, decode_diff_file_name};
use crate::diff_soa::{DiffLineArrays, DiffLineKind};
use crate::diff_word::{
    InFlightHunk, compute_word_diff_spans, parse_git_diff_header_paths, parse_hunk_header,
};

#[derive(Default)]
pub(crate) struct DiffParserState {
    files: Vec<ParsedDiffFile>,
    current: Option<InFlightFile>,
}

struct InFlightFile {
    old_header: Option<String>,
    new_header: Option<String>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    git_old: Option<String>,
    git_new: Option<String>,
    is_new: bool,
    is_deleted: bool,
    additions: usize,
    deletions: usize,
    hunks: Vec<DiffHunkMeta>,
    lines: DiffLineArrays,
    active_hunk: Option<InFlightHunk>,
    pending_deletes: Vec<usize>,
    pending_inserts: Vec<usize>,
    last_line_kind: Option<DiffLineKind>,
}

impl InFlightFile {
    fn new() -> Self {
        Self {
            old_header: None,
            new_header: None,
            rename_from: None,
            rename_to: None,
            git_old: None,
            git_new: None,
            is_new: false,
            is_deleted: false,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
            lines: DiffLineArrays::new(),
            active_hunk: None,
            pending_deletes: Vec::new(),
            pending_inserts: Vec::new(),
            last_line_kind: None,
        }
    }

    fn flush_paired_word_diffs(&mut self) {
        let pairs = self.pending_deletes.len().min(self.pending_inserts.len());
        for i in 0..pairs {
            let del_idx = self.pending_deletes[i];
            let ins_idx = self.pending_inserts[i];
            let (Some(old_text), Some(new_text)) =
                (self.lines.line_text(del_idx), self.lines.line_text(ins_idx))
            else {
                continue;
            };
            let (del_spans, ins_spans) = compute_word_diff_spans(old_text, new_text);
            self.lines.set_line_word_diffs(del_idx, &del_spans);
            self.lines.set_line_word_diffs(ins_idx, &ins_spans);
        }
        self.pending_deletes.clear();
        self.pending_inserts.clear();
    }

    fn close_active_hunk(&mut self) {
        self.flush_paired_word_diffs();
        if let Some(hunk) = self.active_hunk.take() {
            self.hunks.push(hunk.meta);
        }
        self.last_line_kind = None;
    }

    fn finalize(mut self) -> Option<ParsedDiffFile> {
        self.close_active_hunk();
        let old_name = self
            .old_header
            .as_deref()
            .filter(|name| *name != "/dev/null")
            .map(ToOwned::to_owned)
            .or(self.rename_from)
            .or(self.git_old);
        let new_name = self
            .new_header
            .as_deref()
            .filter(|name| *name != "/dev/null")
            .map(ToOwned::to_owned)
            .or(self.rename_to)
            .or(self.git_new);
        let is_new = self.is_new || self.old_header.as_deref() == Some("/dev/null");
        let is_deleted = self.is_deleted || self.new_header.as_deref() == Some("/dev/null");
        let name = new_name.clone().or_else(|| old_name.clone())?;
        let change_type = if is_new {
            DiffChangeType::New
        } else if is_deleted {
            DiffChangeType::Deleted
        } else if old_name.as_deref().is_some_and(|old| old != name) {
            if self.hunks.is_empty() {
                DiffChangeType::RenamePure
            } else {
                DiffChangeType::RenameChanged
            }
        } else {
            DiffChangeType::Change
        };
        let prev_name = match (old_name, is_new) {
            (Some(old), false) if old != name => Some(old),
            _ => None,
        };
        Some(ParsedDiffFile {
            name,
            prev_name,
            change_type,
            additions: self.additions,
            deletions: self.deletions,
            hunks: self.hunks,
            lines: self.lines,
        })
    }
}

impl DiffParserState {
    pub(crate) fn feed_line(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(prev) = self.current.take().and_then(InFlightFile::finalize) {
                self.files.push(prev);
            }
            let mut file = InFlightFile::new();
            let (old_git, new_git) = parse_git_diff_header_paths(rest);
            file.git_old = old_git;
            file.git_new = new_git;
            self.current = Some(file);
            return;
        }
        if let Some(rest) = line.strip_prefix("--- ")
            && self.current.as_ref().is_none_or(|f| {
                f.active_hunk.as_ref().is_none_or(|h| {
                    h.old_remaining == 0 && h.new_remaining == 0 && f.old_header.is_some()
                })
            })
        {
            if self
                .current
                .as_ref()
                .is_some_and(|f| f.old_header.is_some() || f.active_hunk.is_some())
                && let Some(prev) = self.current.take().and_then(InFlightFile::finalize)
            {
                self.files.push(prev);
            }
            let file = self.current.get_or_insert_with(InFlightFile::new);
            file.old_header = Some(decode_diff_file_name(rest, true));
            return;
        }
        let Some(file) = self.current.as_mut() else {
            return;
        };
        if file.active_hunk.is_none() {
            if let Some(rest) = line.strip_prefix("+++ ") {
                file.new_header = Some(decode_diff_file_name(rest, true));
                return;
            }
            if line.starts_with("new file mode ") {
                file.is_new = true;
                return;
            }
            if line.starts_with("deleted file mode ") {
                file.is_deleted = true;
                return;
            }
            if let Some(rest) = line.strip_prefix("rename from ") {
                file.rename_from = Some(decode_diff_file_name(rest, false));
                return;
            }
            if let Some(rest) = line.strip_prefix("rename to ") {
                file.rename_to = Some(decode_diff_file_name(rest, false));
                return;
            }
        }
        if line.starts_with("@@ ")
            && let Some(hunk) = parse_hunk_header(line, file.lines.len())
        {
            file.close_active_hunk();
            file.active_hunk = Some(hunk);
            return;
        }
        if file.active_hunk.is_some() {
            feed_hunk_line(file, line);
        }
    }

    pub(crate) fn finish(mut self) -> Vec<ParsedDiffFile> {
        if let Some(last) = self.current.take().and_then(InFlightFile::finalize) {
            self.files.push(last);
        }
        self.files
    }
}

fn feed_hunk_line(file: &mut InFlightFile, line: &str) {
    let hunk_idx = u32::try_from(file.hunks.len()).unwrap_or(u32::MAX);
    if line == "\\ No newline at end of file" {
        let last_kind = file.last_line_kind;
        if let Some(hunk) = file.active_hunk.as_mut() {
            match last_kind {
                Some(DiffLineKind::Delete) => hunk.meta.no_eof_cr_deletions = true,
                Some(DiffLineKind::Add) => hunk.meta.no_eof_cr_additions = true,
                _ => {}
            }
        }
        return;
    }
    if let Some(added) = line.strip_prefix('+') {
        let line_idx = file.lines.push_line(
            DiffLineKind::Add,
            hunk_idx,
            None,
            file.active_hunk.as_ref().map(|h| h.new_cursor),
            added,
        );
        if let Some(h) = file.active_hunk.as_mut() {
            h.new_cursor = h.new_cursor.saturating_add(1);
            h.new_remaining = h.new_remaining.saturating_sub(1);
            h.meta.line_count += 1;
            h.meta.addition_lines += 1;
        }
        file.additions += 1;
        file.pending_inserts.push(line_idx);
        file.last_line_kind = Some(DiffLineKind::Add);
    } else if let Some(deleted) = line.strip_prefix('-') {
        if !file.pending_inserts.is_empty() {
            file.flush_paired_word_diffs();
        }
        let line_idx = file.lines.push_line(
            DiffLineKind::Delete,
            hunk_idx,
            file.active_hunk.as_ref().map(|h| h.old_cursor),
            None,
            deleted,
        );
        if let Some(h) = file.active_hunk.as_mut() {
            h.old_cursor = h.old_cursor.saturating_add(1);
            h.old_remaining = h.old_remaining.saturating_sub(1);
            h.meta.line_count += 1;
            h.meta.deletion_lines += 1;
        }
        file.deletions += 1;
        file.pending_deletes.push(line_idx);
        file.last_line_kind = Some(DiffLineKind::Delete);
    } else if let Some(ctx) = line.strip_prefix(' ') {
        file.flush_paired_word_diffs();
        if let Some(h) = file.active_hunk.as_mut() {
            file.lines.push_line(
                DiffLineKind::Context,
                hunk_idx,
                Some(h.old_cursor),
                Some(h.new_cursor),
                ctx,
            );
            h.old_cursor = h.old_cursor.saturating_add(1);
            h.new_cursor = h.new_cursor.saturating_add(1);
            h.old_remaining = h.old_remaining.saturating_sub(1);
            h.new_remaining = h.new_remaining.saturating_sub(1);
            h.meta.line_count += 1;
        }
        file.last_line_kind = Some(DiffLineKind::Context);
    }
}

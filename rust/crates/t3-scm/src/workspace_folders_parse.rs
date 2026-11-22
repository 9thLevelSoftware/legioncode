//! `.gitmodules`, lexical containment, and lenient JSONC parsing helpers for
//! [`super::workspace_folders_scanner`].

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

fn gitmodules_path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"^\s*path\s*=\s*(.*?)\s*$");
        // INFALLIBLE: constant `.gitmodules` `path = ...` regex (`60863edc41`).
        compiled.expect("gitmodules_path_regex must compile")
    })
}

/// Extracts submodule paths declared in a `.gitmodules` file, normalized with `/` separators (`60863edc41`).
#[must_use]
pub fn parse_gitmodules_paths(text: &str) -> Vec<String> {
    let re = gitmodules_path_regex();
    text.lines()
        .filter_map(|line| {
            let cap = re.captures(line)?.get(1)?.as_str();
            let normalized = cap
                .replace('"', "")
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_owned();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
}

/// Checks whether `target` sits at or strictly underneath `root` after lexical cleaning.
#[must_use]
pub fn is_within_root(target: &Path, root: &Path) -> bool {
    let clean_target = lexical_clean(target);
    let clean_root = lexical_clean(root);
    clean_target.starts_with(&clean_root)
}

/// Lexically resolves `.` and `..` path components without requiring paths to exist on disk.
#[must_use]
pub fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Strips `//` comments, `/* ... */` block comments, and trailing commas for lenient JSON files.
#[must_use]
pub fn strip_lenient_jsonc(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if in_str {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_str = true;
            out.push(ch);
            i += 1;
        } else if ch == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if ch == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            out.push(ch);
            i += 1;
        }
    }
    remove_trailing_commas(&out)
}

fn remove_trailing_commas(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if in_str {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_str = true;
            out.push(ch);
            i += 1;
        } else if ch == ',' {
            let mut look = i + 1;
            while look < bytes.len() && bytes[look].is_ascii_whitespace() {
                look += 1;
            }
            if look < bytes.len() && matches!(bytes[look], b'}' | b']') {
                i += 1;
            } else {
                out.push(ch);
                i += 1;
            }
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

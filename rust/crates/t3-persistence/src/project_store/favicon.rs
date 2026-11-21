//! Project favicon discovery including Flutter `web/favicon.png` fallback (`3d6336137b`).
//!
//! Mirrors `apps/server/src/project/ProjectFaviconResolver.ts`.

use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};

const ICON_SOURCE_SCAN_BYTES: usize = 128 * 1024;

/// Well-known project favicon paths checked ahead of source HTML/TSX icon declarations.
pub const FAVICON_CANDIDATES: &[&str] = &[
    "favicon.svg",
    "favicon.ico",
    "favicon.png",
    "public/favicon.svg",
    "public/favicon.ico",
    "public/favicon.png",
    "app/favicon.ico",
    "app/favicon.png",
    "app/icon.svg",
    "app/icon.png",
    "app/icon.ico",
    "src/favicon.ico",
    "src/favicon.svg",
    "src/app/favicon.ico",
    "src/app/icon.svg",
    "src/app/icon.png",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    ".idea/icon.svg",
    "Resources/AppIcon.icns",
    "AppIcon.icns",
];

/// Source files inspected for `<link rel="icon">` or route `head` icon declarations.
pub const ICON_SOURCE_FILES: &[&str] = &[
    "index.html",
    "public/index.html",
    "app/routes/__root.tsx",
    "src/routes/__root.tsx",
    "app/root.tsx",
    "src/root.tsx",
    "src/index.html",
];

/// Fallback favicon candidates checked only after source declarations (`3d6336137b`).
pub const FALLBACK_FAVICON_CANDIDATES: &[&str] = &["web/favicon.png"];

fn is_file_within(canonical_root: &Path, candidate: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(candidate).ok()?;
    if !meta.is_file() {
        return None;
    }
    let real_candidate = fs::canonicalize(candidate).ok()?;
    real_candidate
        .starts_with(canonical_root)
        .then_some(real_candidate)
}

fn find_existing_relative(
    canonical_root: &Path,
    project_root: &Path,
    candidates: &[&str],
) -> Option<PathBuf> {
    candidates
        .iter()
        .find_map(|rel| is_file_within(canonical_root, &project_root.join(rel)))
}

fn resolve_declared_href(
    canonical_root: &Path,
    project_root: &Path,
    href: &str,
) -> Option<PathBuf> {
    let trimmed = href.split(['?', '#']).next().unwrap_or("").trim();
    if trimmed.is_empty() || trimmed.contains("://") || trimmed.starts_with("//") {
        return None;
    }
    let stripped = trimmed.trim_start_matches('/').trim_start_matches("./");
    if stripped.is_empty() {
        return None;
    }
    for prefix in ["public", "static", "assets", ""] {
        let rel = if prefix.is_empty() {
            stripped.to_owned()
        } else {
            format!("{prefix}/{stripped}")
        };
        if let Some(found) = is_file_within(canonical_root, &project_root.join(&rel)) {
            return Some(found);
        }
    }
    None
}

fn read_bounded_text(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(u64::try_from(ICON_SOURCE_SCAN_BYTES).unwrap_or(u64::MAX))
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn extract_quoted_attr_ci(haystack: &str, key: &str, assign_char: char) -> Option<String> {
    let lower = haystack.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(pos) = lower[search_from..].find(key) {
        let abs = search_from + pos;
        let before_ok = abs == 0
            || !lower.as_bytes()[abs - 1].is_ascii_alphanumeric()
                && lower.as_bytes()[abs - 1] != b'_';
        let after_key = &haystack[abs + key.len()..];
        let trimmed = after_key.trim_start();
        if before_ok && let Some(rest) = trimmed.strip_prefix(assign_char) {
            let val_part = rest.trim_start();
            let quote = val_part.chars().next()?;
            if (quote == '"' || quote == '\'')
                && let Some(end) = val_part[1..].find(quote)
            {
                return Some(val_part[1..=end].to_owned());
            }
        }
        search_from = abs + key.len();
    }
    None
}

fn scan_source_declared_href(text: &str) -> Option<String> {
    // 1. Scan HTML `<link ...>` tags
    let lower = text.to_ascii_lowercase();
    let mut offset = 0usize;
    while let Some(start_rel) = lower[offset..].find("<link") {
        let tag_start = offset + start_rel;
        let Some(end_rel) = text[tag_start..].find('>') else {
            break;
        };
        let tag = &text[tag_start..=tag_start + end_rel];
        if let Some(rel) = extract_quoted_attr_ci(tag, "rel", '=') {
            let norm_rel = rel.trim().to_ascii_lowercase();
            if (norm_rel == "icon" || norm_rel == "shortcut icon")
                && let Some(href) = extract_quoted_attr_ci(tag, "href", '=')
            {
                return Some(href);
            }
        }
        offset = tag_start + end_rel + 1;
    }

    // 2. Scan JS/TSX `{ rel: "icon", href: "..." }` route link objects
    let mut obj_search = 0usize;
    while let Some(brace_rel) = text[obj_search..].find('{') {
        let start = obj_search + brace_rel;
        if let Some(end_rel) = text[start..].find('}') {
            let block = &text[start..=start + end_rel];
            if let Some(rel) = extract_quoted_attr_ci(block, "rel", ':') {
                let norm_rel = rel.trim().to_ascii_lowercase();
                if (norm_rel == "icon" || norm_rel == "shortcut icon")
                    && let Some(href) = extract_quoted_attr_ci(block, "href", ':')
                {
                    return Some(href);
                }
            }
        }
        obj_search = start + 1;
    }

    None
}

/// Resolves a project's favicon path using candidate priority, source link declarations, and Flutter `web/favicon.png` fallback (`3d6336137b`).
#[must_use]
pub fn resolve_project_favicon(project_root: &Path) -> Option<PathBuf> {
    let canonical_root = fs::canonicalize(project_root).ok()?;
    if !canonical_root.is_dir() {
        return None;
    }

    if let Some(well_known) =
        find_existing_relative(&canonical_root, project_root, FAVICON_CANDIDATES)
    {
        return Some(well_known);
    }

    for source_rel in ICON_SOURCE_FILES {
        let source_path = project_root.join(source_rel);
        let Some(checked) = is_file_within(&canonical_root, &source_path) else {
            continue;
        };
        let Some(text) = read_bounded_text(&checked) else {
            continue;
        };
        if let Some(href) = scan_source_declared_href(&text)
            && let Some(resolved) = resolve_declared_href(&canonical_root, project_root, &href)
        {
            return Some(resolved);
        }
    }

    find_existing_relative(&canonical_root, project_root, FALLBACK_FAVICON_CANDIDATES)
}

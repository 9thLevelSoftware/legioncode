//! Pure HTTP header, byte-range, manifest, and MIME helpers for [`crate::static_assets`].

use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::outbox_state::civil_from_days;

pub(crate) const HTML_CSP: &str = "sandbox allow-scripts allow-forms allow-popups allow-downloads";
pub(crate) const SVG_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParsedByteRange {
    None,
    Satisfiable { first: u64, last: u64 },
    Unsatisfiable,
}

pub(crate) fn content_type_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Percent-decode a URL path, rejecting invalid `%` sequences, NUL bytes, or `..` segments.
pub(crate) fn decode_and_validate_url_path(raw: &str) -> Result<String, &'static str> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'%' => {
                if idx + 2 >= bytes.len() {
                    return Err("truncated percent escape");
                }
                let hi = hex_val(bytes[idx + 1]).ok_or("invalid hex escape")?;
                let lo = hex_val(bytes[idx + 2]).ok_or("invalid hex escape")?;
                let val = (hi << 4) | lo;
                if val == 0 {
                    return Err("NUL byte in path");
                }
                out.push(val);
                idx += 3;
            }
            0 => return Err("NUL byte in path"),
            b => {
                out.push(b);
                idx += 1;
            }
        }
    }
    let decoded = String::from_utf8(out).map_err(|_| "invalid UTF-8 in path")?;
    if decoded.contains('\\') {
        return Err("backslash in path");
    }
    if decoded.split('/').any(|seg| seg == "..") {
        return Err("path traversal segment");
    }
    Ok(decoded)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[derive(Deserialize)]
struct ManifestEntry {
    file: String,
    #[serde(default)]
    css: Option<Vec<String>>,
    #[serde(default)]
    assets: Option<Vec<String>>,
}

pub(crate) fn load_manifest_assets(static_root: &Path) -> HashSet<String> {
    let mut assets = HashSet::new();
    for manifest_path in [
        static_root.join(".vite").join("manifest.json"),
        static_root.join("manifest.json"),
    ] {
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(map) =
            serde_json::from_slice::<std::collections::HashMap<String, ManifestEntry>>(&bytes)
        else {
            continue;
        };
        for entry in map.into_values() {
            for rel in std::iter::once(entry.file)
                .chain(entry.css.unwrap_or_default())
                .chain(entry.assets.unwrap_or_default())
            {
                let trimmed = rel.trim_start_matches('/');
                if !trimmed.is_empty() {
                    assets.insert(trimmed.to_owned());
                }
            }
        }
    }
    assets
}

pub(crate) fn build_weak_etag(size: u64, mtime: SystemTime) -> String {
    let ms = mtime
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("W/\"{size:x}-{ms:x}\"")
}

pub(crate) fn format_http_date_unix_secs(secs: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;
    let dow_idx = usize::try_from(days.rem_euclid(7)).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    let mon_idx = usize::try_from(month - 1).unwrap_or(0);
    let dow = DAYS.get(dow_idx).copied().unwrap_or("Thu");
    let mon = MONTHS.get(mon_idx).copied().unwrap_or("Jan");
    format!("{dow}, {day:02} {mon} {year:04} {hour:02}:{minute:02}:{second:02} GMT")
}

pub(crate) fn system_time_unix_secs(t: SystemTime) -> i64 {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    i64::try_from(secs).unwrap_or(i64::MAX)
}

pub(crate) fn if_none_match_matches(header: &str, etag: &str) -> bool {
    let target = strip_weak_prefix(etag);
    header.split(',').any(|candidate| {
        let trimmed = candidate.trim();
        trimmed == "*" || strip_weak_prefix(trimmed) == target
    })
}

fn strip_weak_prefix(tag: &str) -> &str {
    tag.trim()
        .strip_prefix("W/")
        .or_else(|| tag.trim().strip_prefix("w/"))
        .unwrap_or(tag.trim())
        .trim()
}

pub(crate) fn parse_byte_range(header: Option<&str>, size: u64) -> ParsedByteRange {
    let Some(raw) = header.map(str::trim) else {
        return ParsedByteRange::None;
    };
    let Some(spec) = raw
        .strip_prefix("bytes=")
        .or_else(|| raw.strip_prefix("BYTES="))
    else {
        return ParsedByteRange::None;
    };
    let Some((start_s, end_s)) = spec.split_once('-') else {
        return ParsedByteRange::None;
    };
    if end_s.contains(',') {
        return ParsedByteRange::None;
    }
    let start_s = start_s.trim();
    let end_s = end_s.trim();
    if start_s.is_empty() && end_s.is_empty() {
        return ParsedByteRange::None;
    }
    if size == 0 {
        return ParsedByteRange::Unsatisfiable;
    }
    if start_s.is_empty() {
        let Ok(suffix) = end_s.parse::<u64>() else {
            return ParsedByteRange::None;
        };
        if suffix == 0 {
            return ParsedByteRange::Unsatisfiable;
        }
        let first = size.saturating_sub(suffix);
        return ParsedByteRange::Satisfiable {
            first,
            last: size - 1,
        };
    }
    let Ok(first) = start_s.parse::<u64>() else {
        return ParsedByteRange::None;
    };
    if first >= size {
        return ParsedByteRange::Unsatisfiable;
    }
    if end_s.is_empty() {
        return ParsedByteRange::Satisfiable {
            first,
            last: size - 1,
        };
    }
    let Ok(end) = end_s.parse::<u64>() else {
        return ParsedByteRange::None;
    };
    if end < first {
        return ParsedByteRange::None;
    }
    ParsedByteRange::Satisfiable {
        first,
        last: end.min(size - 1),
    }
}

const WORKSPACE_PREVIEW_EXTENSIONS: &[&str] = &[
    "htm", "html", "pdf", "avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "svg", "webp", "ttf",
    "otf", "woff", "woff2", "zip", "jar", "tar", "gz", "tgz", "sqlite", "sqlite3", "db", "docx",
    "xlsx", "pptx", "odt", "ods", "odp", "rtf", "epub", "ipynb", "mp3", "wav", "ogg", "m4a", "aac",
    "flac", "opus", "weba", "mp4", "webm", "m4v", "ogv", "mov",
];

/// Returns `true` when `path` has an extension supported by workspace file/browser previews (`#39491`).
///
/// Mirrors `@legioncode/shared/filePreview`'s `isWorkspacePreviewEntryPath`: excludes unsupported XML/XHTML
/// variants such as `.xhtml` while allowing browser previews (`.htm`, `.html`, `.pdf`), images, fonts,
/// media, office documents, archives, and sqlite files.
pub fn is_workspace_preview_entry_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    WORKSPACE_PREVIEW_EXTENSIONS.contains(&lower.as_str())
}

/// Resolves a workspace asset (following symlinks) ONLY when its canonical target exists as a
/// regular file inside at least one canonical entry in `allowed_roots` AND satisfies
/// [`is_workspace_preview_entry_path`] (`#39491`).
pub fn resolve_previewable_workspace_asset(
    candidate: &Path,
    allowed_roots: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    if !is_workspace_preview_entry_path(candidate) {
        return None;
    }
    let canonical_target = std::fs::canonicalize(candidate).ok()?;
    if !canonical_target.is_file() {
        return None;
    }
    if !is_workspace_preview_entry_path(&canonical_target) {
        return None;
    }
    let within_allowed = allowed_roots.iter().any(|root| {
        std::fs::canonicalize(root)
            .is_ok_and(|canonical_root| canonical_target.starts_with(&canonical_root))
    });
    within_allowed.then_some(canonical_target)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use reqwest::StatusCode;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::{HTML_CSP, load_manifest_assets};
    use crate::static_assets::static_assets_router;

    #[test]
    fn load_manifest_assets_collects_file_css_and_assets_from_both_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir_a = tempfile::tempdir()?;
        fs::create_dir_all(dir_a.path().join(".vite"))?;
        fs::write(
            dir_a.path().join(".vite").join("manifest.json"),
            r#"{"src/main.ts":{"file":"/assets/main-1111aaaa.js","css":["assets/main-2222bbbb.css"],"assets":["assets/font-3333cccc.woff2"]}}"#,
        )?;
        let assets_a = load_manifest_assets(dir_a.path());
        assert!(assets_a.contains("assets/main-1111aaaa.js"));
        assert!(assets_a.contains("assets/main-2222bbbb.css"));
        assert!(assets_a.contains("assets/font-3333cccc.woff2"));

        let dir_b = tempfile::tempdir()?;
        fs::write(
            dir_b.path().join("manifest.json"),
            r#"{"index.html":{"file":"assets/root-4444dddd.js","css":["/assets/root-5555eeee.css"]}}"#,
        )?;
        let assets_b = load_manifest_assets(dir_b.path());
        assert!(assets_b.contains("assets/root-4444dddd.js"));
        assert!(assets_b.contains("assets/root-5555eeee.css"));
        Ok(())
    }

    fn hdr<'a>(resp: &'a reqwest::Response, name: &str) -> Option<&'a str> {
        resp.headers().get(name).and_then(|v| v.to_str().ok())
    }

    #[tokio::test]
    async fn static_assets_blocks_reserved_api_ws_oauth_well_known_fallbacks_and_sets_csp()
    -> Result<(), Box<dyn std::error::Error>> {
        let dist = tempfile::tempdir()?;
        let root = dist.path();
        fs::create_dir_all(root.join(".vite"))?;
        fs::create_dir_all(root.join(".well-known"))?;
        fs::create_dir_all(root.join("assets"))?;
        fs::create_dir_all(root.join("preview"))?;
        fs::write(root.join("index.html"), "<!doctype html><html>SPA</html>")?;
        fs::write(root.join("preview").join("index.html"), "<html>Sub</html>")?;
        fs::write(root.join("assets").join("app-12345678.css"), "body{}")?;
        fs::write(root.join(".well-known").join("assetlinks.json"), "[]")?;
        fs::write(
            root.join(".vite").join("manifest.json"),
            r#"{"index.html":{"file":"assets/app-12345678.js","css":["assets/app-12345678.css"]}}"#,
        )?;

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let router = static_assets_router(Some(root.to_path_buf()));
        let _srv = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let client = reqwest::Client::new();
        for missing in [
            "/api/unknown",
            "/ws",
            "/ws/unknown",
            "/oauth/token",
            "/.well-known/unknown",
        ] {
            let resp = client.get(format!("{base_url}{missing}")).send().await?;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "path {missing}");
            assert_eq!(resp.json::<Value>().await?, json!({ "error": "Not Found" }));
        }

        let wk = client
            .get(format!("{base_url}/.well-known/assetlinks.json"))
            .send()
            .await?;
        assert_eq!(wk.status(), StatusCode::OK);

        let css = client
            .get(format!("{base_url}/assets/app-12345678.css"))
            .send()
            .await?;
        assert_eq!(
            hdr(&css, "cache-control"),
            Some("public, max-age=31536000, immutable")
        );

        let spa = client.get(format!("{base_url}/threads/123")).send().await?;
        assert_eq!(spa.status(), StatusCode::OK);
        assert_eq!(hdr(&spa, "x-frame-options"), Some("DENY"));
        assert_eq!(
            hdr(&spa, "content-security-policy"),
            Some("frame-ancestors 'none'")
        );

        let preview = client.get(format!("{base_url}/preview")).send().await?;
        assert_eq!(preview.status(), StatusCode::OK);
        assert_eq!(hdr(&preview, "content-security-policy"), Some(HTML_CSP));
        assert!(hdr(&preview, "x-frame-options").is_none());
        Ok(())
    }
}

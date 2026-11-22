//! Custom protocol URI normalization (`legioncode://app/*` and Windows `http://legioncode.localhost/*`),
//! `DesktopSchemeRouter` dispatch with `X-Content-Type-Options: nosniff`, and smoke-test HTML.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

use http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use http::{Request, Response, StatusCode};
use legion_surfaces::desktop::{
    DESKTOP_HOST, DESKTOP_PRODUCTION_SCHEME, DesktopRouteDecision, DesktopSchemeRouter,
};

const SMOKE_ASSET_JS: &str = "window.__legionSmokeAssetLoaded = true;\n";
const DEFAULT_SPA_FALLBACK_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>LegionCode</title></head><body><div id=\"root\"></div></body></html>\n";

/// Normalizes Windows `WebView2` `http(s)://legioncode.localhost/*` (or `legioncode://localhost/*` after
/// `wry` workaround reversal) into canonical `legioncode://app/*` while keeping foreign origins raw
/// so [`DesktopSchemeRouter`] rejects them with `404` `ForeignOrigin`.
pub fn normalize_custom_protocol_url(scheme: &str, raw_uri: &str) -> String {
    if raw_uri.starts_with('/') {
        return format!("{scheme}://{DESKTOP_HOST}{raw_uri}");
    }
    let Some((raw_scheme, rest)) = raw_uri.split_once("://") else {
        return raw_uri.to_owned();
    };

    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    if raw_scheme.eq_ignore_ascii_case("http") || raw_scheme.eq_ignore_ascii_case("https") {
        let localhost_alias = format!("{scheme}.localhost");
        let app_alias = format!("{scheme}.{DESKTOP_HOST}");
        if authority.eq_ignore_ascii_case(&localhost_alias)
            || authority.eq_ignore_ascii_case(&app_alias)
        {
            return format!("{scheme}://{DESKTOP_HOST}{tail}");
        }
        return raw_uri.to_owned();
    }

    if raw_scheme.eq_ignore_ascii_case(scheme) && authority.eq_ignore_ascii_case("localhost") {
        return format!("{scheme}://{DESKTOP_HOST}{tail}");
    }

    raw_uri.to_owned()
}

/// Callback counters updated when custom protocol requests execute inside a `WebView`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CustomProtocolMetrics {
    /// At least one `200` document or asset response was served over the custom scheme.
    pub served_ok: bool,
    /// At least one traversal (`..` / `%2e%2e` / `\`) or foreign origin returned `404`.
    pub traversal_rejected: bool,
}

/// Resolves an incoming `wry` custom-protocol HTTP request through [`DesktopSchemeRouter`]
/// and attaches `Content-Security-Policy` + `X-Content-Type-Options: nosniff`.
pub fn resolve_custom_protocol_request(
    scheme: &str,
    clerk_host: Option<&str>,
    static_dir: Option<&Path>,
    smoke_html: Option<&str>,
    request: &Request<Vec<u8>>,
    metrics: Option<&mut CustomProtocolMetrics>,
) -> Response<Cow<'static, [u8]>> {
    let raw_uri = request.uri().to_string();
    let normalized = normalize_custom_protocol_url(scheme, &raw_uri);
    let accept = request
        .headers()
        .get("accept")
        .and_then(|val| val.to_str().ok());

    let mut router = DesktopSchemeRouter::new(scheme, clerk_host);
    if smoke_html.is_some() {
        router = router.with_assets(["index.html", "assets/smoke.js"]);
    }

    let decision = router.route_with(request.method().as_str(), &normalized, accept, |rel| {
        if smoke_html.is_some() && matches!(rel, "index.html" | "assets/smoke.js") {
            return true;
        }
        static_dir
            .and_then(|root| safe_static_asset_path(root, rel))
            .is_some_and(|path| path.is_file())
    });

    match decision {
        DesktopRouteDecision::ServeAsset {
            relative_path,
            mime_type,
            content_security_policy,
            head_only,
        } => {
            if let Some(bytes) = load_asset_bytes(&relative_path, static_dir, smoke_html) {
                if let Some(m) = metrics {
                    m.served_ok = true;
                }
                let body = if head_only {
                    Cow::Borrowed(&[][..])
                } else {
                    bytes
                };
                return build_protocol_response(
                    StatusCode::OK,
                    mime_type,
                    &content_security_policy,
                    body,
                );
            }
            if let Some(m) = metrics {
                m.traversal_rejected = true;
            }
            build_json_error_response(StatusCode::NOT_FOUND, "not_found", &content_security_policy)
        }
        DesktopRouteDecision::FallbackSpaIndex {
            relative_path,
            mime_type,
            content_security_policy,
            head_only,
        } => {
            if let Some(m) = metrics {
                m.served_ok = true;
            }
            let body = if head_only {
                Cow::Borrowed(&[][..])
            } else {
                load_asset_bytes(relative_path, static_dir, smoke_html)
                    .unwrap_or_else(|| Cow::Borrowed(DEFAULT_SPA_FALLBACK_HTML.as_bytes()))
            };
            build_protocol_response(StatusCode::OK, mime_type, &content_security_policy, body)
        }
        DesktopRouteDecision::Rejected {
            status,
            reason,
            content_security_policy,
        } => {
            if let Some(m) = metrics
                && status == 404
            {
                m.traversal_rejected = true;
            }
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
            let reason_tag = serde_json::to_value(reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "rejected".to_owned());
            build_json_error_response(code, &reason_tag, &content_security_policy)
        }
    }
}

fn load_asset_bytes(
    relative_path: &str,
    static_dir: Option<&Path>,
    smoke_html: Option<&str>,
) -> Option<Cow<'static, [u8]>> {
    if let Some(root) = static_dir
        && let Some(path) = safe_static_asset_path(root, relative_path)
        && let Ok(bytes) = fs::read(path)
    {
        return Some(Cow::Owned(bytes));
    }
    if let Some(html) = smoke_html {
        if relative_path == "index.html" {
            return Some(Cow::Owned(html.as_bytes().to_vec()));
        }
        if relative_path == "assets/smoke.js" {
            return Some(Cow::Borrowed(SMOKE_ASSET_JS.as_bytes()));
        }
    }
    None
}

fn safe_static_asset_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut candidate = root.to_path_buf();
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains('\\') {
            return None;
        }
        candidate.push(segment);
    }
    if let (Ok(canon_root), Ok(canon_file)) = (fs::canonicalize(root), fs::canonicalize(&candidate))
        && !canon_file.starts_with(&canon_root)
    {
        return None;
    }
    Some(candidate)
}

fn build_protocol_response(
    status: StatusCode,
    content_type: &str,
    csp: &str,
    body: Cow<'static, [u8]>,
) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_SECURITY_POLICY, csp)
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(body)
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[][..])))
}

fn build_json_error_response(
    status: StatusCode,
    reason: &str,
    csp: &str,
) -> Response<Cow<'static, [u8]>> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": reason })).unwrap_or_default();
    build_protocol_response(
        status,
        "application/json; charset=utf-8",
        csp,
        Cow::Owned(body),
    )
}

/// Self-contained boot HTML loaded over `legioncode://app/` in `--smoke-webview` mode.
pub fn build_smoke_html() -> &'static str {
    r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>LegionCode Smoke</title></head>
<body>
<pre id="status">running</pre>
<script>
(() => {
  const pending = new Map();
  window.__legionReceiveIpc = (raw) => {
    const msg = typeof raw === "string" ? JSON.parse(raw) : raw;
    const entry = pending.get(msg.id);
    if (entry) {
      pending.delete(msg.id);
      entry(msg);
    }
  };
  const invoke = (id, method, params) => new Promise((resolve) => {
    pending.set(id, resolve);
    window.ipc.postMessage(JSON.stringify({ id, method, params }));
  });
  (async () => {
    const valid = await invoke("r1", "desktop.resolveScheme", {
      method: "GET",
      url: "legioncode://app/assets/smoke.js",
      assets: ["index.html", "assets/smoke.js"]
    });
    const traversal = await invoke("r2", "desktop.resolveScheme", {
      method: "GET",
      url: "legioncode://app/%2e%2e%2fetc/passwd"
    });
    const bounds = await invoke("r3", "desktop.clampWindowBounds", {
      bounds: { x: 9000, y: -500, width: 3000, height: 200 },
      displays: [{ x: 0, y: 0, width: 1920, height: 1080 }]
    });
    let fetchOk = false;
    let fetchTraversalStatus = 0;
    try {
      const assetRes = await fetch("/assets/smoke.js");
      fetchOk = assetRes.status === 200;
      const badRes = await fetch("/%2e%2e%2fetc/passwd");
      fetchTraversalStatus = badRes.status;
    } catch {
      fetchOk = false;
    }
    const ok = Boolean(
      valid?.ok && valid.result?.status === 200 &&
      traversal?.ok && traversal.result?.status === 404 &&
      bounds?.ok && bounds.result?.clamped?.width === 1920 &&
      bounds.result?.clamped?.height === 360
    );
    document.getElementById("status").textContent = ok ? "passed" : "failed";
    window.ipc.postMessage(JSON.stringify({
      id: "done",
      method: "desktop.__smokeComplete",
      params: {
        ok,
        traversalRejected: traversal?.result?.status === 404,
        fetchOk,
        fetchTraversalStatus
      }
    }));
  })();
})();
</script>
</body>
</html>
"#
}

pub(crate) const fn desktop_scheme_name() -> &'static str {
    let active = legion_contracts::brand::ACTIVE_BRAND.url_scheme;
    if active.is_empty() {
        DESKTOP_PRODUCTION_SCHEME
    } else {
        active
    }
}

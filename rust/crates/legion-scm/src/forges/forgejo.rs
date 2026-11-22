//! `Forgejo` adapter (`keys.json` credential path, `fj`/`tea` CLI, WIP heuristic, required check globs).

use std::sync::OnceLock;

use http::Method;
use legion_contracts::pull_requests::{PullRequestChecksState, PullRequestMergeability};
use regex::Regex;
use serde_json::Value;

use crate::core::pr_helpers::{CheckEntry, rollup_check_statuses};
use crate::forges::{
    CliCommandSpec, ForgeError, HttpRequestSpec, NormalizedPullRequest, json_str, json_u64,
    parse_standard_pr_state, validate_hostname,
};

/// Host operating system selector for resolving `Forgejo` CLI `keys.json` config path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    /// `macOS` (`~/Library/Application Support/forgejo-cli/keys.json`).
    MacOs,
    /// `Windows` (`%APPDATA%\forgejo-cli\keys.json`).
    Windows,
    /// `Linux` / `BSD` (`$XDG_CONFIG_HOME/forgejo-cli/keys.json` or `~/.config/...`).
    Linux,
}

fn tea_http_status_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let compiled = Regex::new(r"HTTP/\S+\s+(\d{3})");
        // INFALLIBLE: static regex for `tea api --include` HTTP status header line.
        compiled.expect("valid tea HTTP status regex")
    })
}

/// Resolves the `forgejo-cli` (`fj`) `keys.json` path across platforms.
///
/// ```
/// use legion_scm::forges::forgejo::{TargetOs, forgejo_keys_json_path};
///
/// assert_eq!(
///     forgejo_keys_json_path(TargetOs::MacOs, "/Users/alice", None, None),
///     "/Users/alice/Library/Application Support/forgejo-cli/keys.json"
/// );
/// assert_eq!(
///     forgejo_keys_json_path(TargetOs::Linux, "/home/alice", None, Some("/cfg")),
///     "/cfg/forgejo-cli/keys.json"
/// );
/// ```
#[must_use]
pub fn forgejo_keys_json_path(
    os: TargetOs,
    home_dir: &str,
    appdata: Option<&str>,
    xdg_config_home: Option<&str>,
) -> String {
    match os {
        TargetOs::MacOs => format!(
            "{}/Library/Application Support/forgejo-cli/keys.json",
            home_dir.trim_end_matches('/')
        ),
        TargetOs::Windows => {
            let base = appdata.filter(|s| !s.trim().is_empty()).map_or_else(
                || {
                    format!(
                        "{}\\AppData\\Roaming",
                        home_dir.trim_end_matches(['\\', '/'])
                    )
                },
                |s| s.trim_end_matches(['\\', '/']).to_owned(),
            );
            format!("{base}\\forgejo-cli\\keys.json")
        }
        TargetOs::Linux => {
            let base = xdg_config_home
                .filter(|s| !s.trim().is_empty())
                .map_or_else(
                    || format!("{}/.config", home_dir.trim_end_matches('/')),
                    |s| s.trim_end_matches('/').to_owned(),
                );
            format!("{base}/forgejo-cli/keys.json")
        }
    }
}

/// Builds a direct `Forgejo` `/api/v1` REST request using `Authorization: token <token>`.
///
/// # Errors
///
/// Returns [`ForgeError::InvalidHost`] if `hostname` is malformed or [`ForgeError::InvalidCredentials`]
/// if `token` is empty/contains line breaks.
pub fn build_forgejo_http_request(
    hostname: &str,
    token: &str,
    method: Method,
    api_path: &str,
    body: Option<Value>,
) -> Result<HttpRequestSpec, ForgeError> {
    let host = validate_hostname(hostname)?;
    let trimmed_tok = token.trim();
    if trimmed_tok.is_empty() || trimmed_tok.contains(['\r', '\n']) {
        return Err(ForgeError::InvalidCredentials(
            "Forgejo token is empty or contains line breaks".to_owned(),
        ));
    }
    let normalized_path = if api_path.starts_with("/api/v1") {
        api_path.to_owned()
    } else if api_path.starts_with('/') {
        format!("/api/v1{api_path}")
    } else {
        format!("/api/v1/{api_path}")
    };
    let mut headers = vec![
        ("authorization".to_owned(), format!("token {trimmed_tok}")),
        ("accept".to_owned(), "application/json".to_owned()),
    ];
    let body_str = body.map(|v| {
        headers.push(("content-type".to_owned(), "application/json".to_owned()));
        v.to_string()
    });
    Ok(HttpRequestSpec {
        method,
        url: format!("https://{host}{normalized_path}"),
        headers,
        body: body_str,
    })
}

/// Builds a `tea api --include` CLI invocation for fallback `Gitea`/`Forgejo` hosts.
#[must_use]
pub fn build_tea_api_command(
    login_name: Option<&str>,
    method: &Method,
    endpoint: &str,
    json_data: Option<&str>,
) -> CliCommandSpec {
    let mut args = vec![
        "api".to_owned(),
        "--include".to_owned(),
        "--method".to_owned(),
        method.as_str().to_owned(),
    ];
    if let Some(login) = login_name
        && !login.trim().is_empty()
    {
        args.push("--login".to_owned());
        args.push(login.trim().to_owned());
    }
    if let Some(data) = json_data {
        args.push("--data".to_owned());
        args.push(data.to_owned());
    }
    args.push(endpoint.to_owned());

    CliCommandSpec {
        program: "tea".to_owned(),
        args,
        env: Vec::new(),
        stdin: None,
    }
}

/// Extracts the HTTP status code emitted on `stderr` by `tea api --include`.
#[must_use]
pub fn parse_tea_http_status(stderr: &str) -> Option<u16> {
    tea_http_status_regex()
        .captures(stderr)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u16>().ok())
}

/// Computes the checkout branch or synthetic pull ref for a `Forgejo` PR.
///
/// Upstream `agit` workflow PRs (`flow == Some(1)`) or unlabeled fork PRs resolve to `refs/pull/<n>/head`.
#[must_use]
pub fn forgejo_head_branch(pr_number: u64, head_label: Option<&str>, flow: Option<u64>) -> String {
    if flow != Some(1)
        && let Some(label) = head_label.map(str::trim)
        && !label.is_empty()
    {
        return label.to_owned();
    }
    format!("refs/pull/{pr_number}/head")
}

/// Returns `true` when `title` starts with `[WIP]` or `WIP:` (case-insensitive).
#[must_use]
pub fn is_forgejo_wip_title(title: &str) -> bool {
    let lower = title.trim_start().to_ascii_lowercase();
    lower.starts_with("[wip]") || lower.starts_with("wip:")
}

/// Marks matching checks as `required = true` and returns `true` when every `required_pattern`
/// matched at least one existing check.
///
/// Supports linear globs (`*` and `?`). Patterns containing regex/class escapes (`[`, `]`, `{`, `}`, `\`)
/// never match so unsupported branch protection patterns stay safely pending/unmet.
#[must_use]
pub fn mark_required_forgejo_checks(checks: &mut [CheckEntry], required_patterns: &[&str]) -> bool {
    let mut all_matched = true;
    for pattern in required_patterns {
        let mut pattern_matched = false;
        if !has_unsupported_glob_syntax(pattern) {
            for check in checks.iter_mut() {
                if glob_match_linear(pattern, &check.name) {
                    check.required = true;
                    pattern_matched = true;
                }
            }
        }
        if !pattern_matched {
            all_matched = false;
        }
    }
    all_matched
}

/// Normalizes a `Forgejo` pull request JSON payload into [`NormalizedPullRequest`].
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if required fields (`number`, `title`, branches) are missing.
pub fn normalize_forgejo_pr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number = json_u64(json, "number")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Forgejo PR `number`".to_owned()))?;
    let title = json_str(json, "title")
        .ok_or_else(|| ForgeError::MalformedResponse("missing Forgejo PR `title`".to_owned()))?
        .to_owned();
    let url = json_str(json, "html_url")
        .or_else(|| json_str(json, "url"))
        .unwrap_or("")
        .to_owned();

    let is_merged = json.get("merged").and_then(Value::as_bool) == Some(true)
        || json.get("merged_at").is_some_and(|v| !v.is_null());
    let state = parse_standard_pr_state(json_str(json, "state").unwrap_or("open"), is_merged);
    let is_draft =
        is_forgejo_wip_title(&title) || json.get("draft").and_then(Value::as_bool) == Some(true);

    let flow = json_u64(json, "flow");
    let head_label = json.get("head").and_then(|h| json_str(h, "label"));
    let head_ref = json.get("head").and_then(|h| json_str(h, "ref"));
    let head_branch = if head_label.is_some() || flow == Some(1) {
        forgejo_head_branch(number, head_ref.or(head_label), flow)
    } else if let Some(r) = head_ref {
        r.to_owned()
    } else {
        return Err(ForgeError::MalformedResponse(
            "missing Forgejo PR `head` branch".to_owned(),
        ));
    };

    let base_branch = json
        .get("base")
        .and_then(|b| json_str(b, "ref"))
        .ok_or_else(|| ForgeError::MalformedResponse("missing Forgejo `base.ref`".to_owned()))?
        .to_owned();
    let head_sha = json
        .get("head")
        .and_then(|h| json_str(h, "sha"))
        .map(str::to_owned);

    let mergeability = match json.get("mergeable").and_then(Value::as_bool) {
        Some(true) => PullRequestMergeability::Mergeable,
        Some(false) => PullRequestMergeability::Conflicting,
        None => PullRequestMergeability::Unknown,
    };

    Ok(NormalizedPullRequest {
        number,
        title,
        url,
        state,
        is_draft,
        head_branch,
        base_branch,
        head_sha,
        mergeability,
        checks_state: rollup_check_statuses(&[]),
    })
}

fn has_unsupported_glob_syntax(pattern: &str) -> bool {
    pattern.contains(['[', ']', '{', '}', '\\'])
}

/// Two-pointer linear wildcard match supporting `*` and `?` without backtracking blowups.
fn glob_match_linear(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_pi, mut match_ti) = (None, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            match_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            match_ti += 1;
            ti = match_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Convenience check state default when caller supplies empty slice.
#[must_use]
pub fn empty_checks_state() -> PullRequestChecksState {
    rollup_check_statuses(&[])
}

//! `Cursor` `Origin` adapter (`origin.cursor.com`, Int64-string decoding, SHA-pinned mergeability).

use serde_json::Value;
use t3_contracts::pull_requests::{PullRequestChecksState, PullRequestMergeability};

use crate::forges::{
    CliCommandSpec, ForgeError, NormalizedPullRequest, json_str, parse_standard_pr_state,
};

/// Primary `Cursor` `Origin` web host.
pub const CURSOR_ORIGIN_HOST: &str = "origin.cursor.com";

/// Canonical `Cursor` codebase web URL prefix.
pub const CURSOR_CODEBASE_PREFIX: &str = "https://cursor.com/codebase";

/// Canonical `Cursor` API origin.
pub const CURSOR_API_ORIGIN: &str = "https://api.cursor.com";

/// Returns `true` when `remote_url` or `host` targets `origin.cursor.com`, `cursor.com/codebase`,
/// or `api.cursor.com`.
#[must_use]
pub fn is_cursor_origin_target(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    lower.contains(CURSOR_ORIGIN_HOST)
        || lower.contains("cursor.com/codebase")
        || lower.contains("api.cursor.com")
}

/// Decodes an upstream Protobuf `int64` encoded as either a JSON number or decimal string.
///
/// ```
/// use serde_json::json;
/// use t3_scm::forges::cursor_origin::decode_i64_flexible;
///
/// assert_eq!(decode_i64_flexible(&json!("9007199254740993")), Some(9_007_199_254_740_993));
/// assert_eq!(decode_i64_flexible(&json!(42)), Some(42));
/// assert_eq!(decode_i64_flexible(&json!(true)), None);
/// ```
#[must_use]
pub fn decode_i64_flexible(value: &Value) -> Option<i64> {
    match value {
        Value::Number(num) => num.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Computes `Cursor` `Origin`'s SHA-pinned `preparedMergeability`: returns [`PullRequestMergeability::Unknown`]
/// unless `version_head_sha` matches `current_head_sha` and `raw_mergeable` is present.
#[must_use]
pub fn prepared_mergeability(
    version_head_sha: Option<&str>,
    current_head_sha: Option<&str>,
    raw_mergeable: Option<bool>,
) -> PullRequestMergeability {
    let (Some(ver_sha), Some(cur_sha), Some(mergeable)) =
        (version_head_sha, current_head_sha, raw_mergeable)
    else {
        return PullRequestMergeability::Unknown;
    };
    if ver_sha != cur_sha || cur_sha.trim().is_empty() {
        return PullRequestMergeability::Unknown;
    }
    if mergeable {
        PullRequestMergeability::Mergeable
    } else {
        PullRequestMergeability::Conflicting
    }
}

/// Builds an `origin` CLI command invocation.
#[must_use]
pub fn build_origin_cli_command(args: &[&str], stdin: Option<String>) -> CliCommandSpec {
    CliCommandSpec {
        program: "origin".to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        env: Vec::new(),
        stdin,
    }
}

/// Normalizes a `Cursor` `Origin` change/PR JSON payload into [`NormalizedPullRequest`].
///
/// Handles Protobuf JSON `number` / `id` serialized as strings or integers and checks
/// `preparedMergeability` against `headSha`.
///
/// # Errors
///
/// Returns [`ForgeError::MalformedResponse`] if numeric identifier, `title`, or branch names
/// cannot be extracted.
pub fn normalize_cursor_origin_pr_json(json: &Value) -> Result<NormalizedPullRequest, ForgeError> {
    let number_i64 = json
        .get("number")
        .and_then(decode_i64_flexible)
        .or_else(|| json.get("id").and_then(decode_i64_flexible))
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Cursor Origin PR `number`/`id`".to_owned())
        })?;
    let number = u64::try_from(number_i64).map_err(|_| {
        ForgeError::MalformedResponse(format!("negative Cursor Origin PR number `{number_i64}`"))
    })?;
    let title = json_str(json, "title")
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Cursor Origin PR `title`".to_owned())
        })?
        .to_owned();
    let url = json_str(json, "url").unwrap_or("").to_owned();

    let is_merged = json.get("merged").and_then(Value::as_bool) == Some(true)
        || json_str(json, "status").is_some_and(|s| s.eq_ignore_ascii_case("merged"));
    let state = parse_standard_pr_state(
        json_str(json, "state")
            .or_else(|| json_str(json, "status"))
            .unwrap_or("open"),
        is_merged,
    );
    let is_draft = json
        .get("isDraft")
        .and_then(Value::as_bool)
        .or_else(|| json.get("draft").and_then(Value::as_bool))
        .unwrap_or(false);

    let head_branch = json_str(json, "headBranch")
        .or_else(|| json_str(json, "sourceBranch"))
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Cursor Origin PR `headBranch`".to_owned())
        })?
        .to_owned();
    let base_branch = json_str(json, "baseBranch")
        .or_else(|| json_str(json, "targetBranch"))
        .ok_or_else(|| {
            ForgeError::MalformedResponse("missing Cursor Origin PR `baseBranch`".to_owned())
        })?
        .to_owned();

    let head_sha = json_str(json, "headSha").map(str::to_owned);
    let version_head_sha = json_str(json, "preparedVersionHeadSha").or_else(|| {
        json.get("preparedMergeability")
            .and_then(|p| json_str(p, "versionHeadSha"))
    });
    let raw_mergeable = json
        .get("preparedMergeability")
        .and_then(|p| p.get("mergeable").and_then(Value::as_bool))
        .or_else(|| json.get("mergeable").and_then(Value::as_bool));
    let mergeability = prepared_mergeability(version_head_sha, head_sha.as_deref(), raw_mergeable);

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
        checks_state: PullRequestChecksState::Pending,
    })
}

//! `Azure` `DevOps` PAT / token auth verification (`da60db38e8`, `#33864`, `#24170`).

use http::Method;
use serde_json::Value;

use crate::core::discovery::ParsedRemote;
use crate::forges::azure_devops::AzureCliLaunch;
use crate::forges::{CliCommandSpec, ForgeError, HttpRequestSpec, json_str};

/// Verified `Azure` `DevOps` viewer returned by Location `connectionData` auth probing (`da60db38e8`, `#24170`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureAuthenticatedViewer {
    /// Authenticated identity UUID/descriptor.
    pub id: String,
    /// Authenticated viewer display name (`customDisplayName` or `providerDisplayName`).
    pub display_name: Option<String>,
}

/// Encodes `Authorization: Basic <base64(:pat)>` for `Azure` `DevOps` PAT REST auth (`da60db38e8`).
pub fn encode_azure_pat_basic_auth(pat: &str) -> Result<String, ForgeError> {
    let trimmed = validate_azure_pat(pat)?;
    let raw = format!(":{trimmed}");
    Ok(format!("Basic {}", encode_base64(raw.as_bytes())))
}

/// Appends `AZURE_DEVOPS_EXT_PAT` to `launch.env` after rejecting CR/LF/empty PAT values (`#33864`).
pub fn azure_pat_cli_env(
    launch: &AzureCliLaunch,
    pat: &str,
) -> Result<Vec<(String, String)>, ForgeError> {
    let trimmed = validate_azure_pat(pat)?;
    let mut env = launch.env.clone();
    env.push(("AZURE_DEVOPS_EXT_PAT".to_owned(), trimmed.to_owned()));
    Ok(env)
}

/// Builds an HTTP GET `connectionData` request verifying an `Azure` `DevOps` PAT (`da60db38e8`).
pub fn build_azure_connection_data_http_request(
    organization: &str,
    token: &str,
) -> Result<HttpRequestSpec, ForgeError> {
    let org = organization.trim().trim_matches('/');
    if org.is_empty() || org.contains(['/', '\\', '?', '#']) {
        return Err(ForgeError::InvalidHost {
            host: organization.to_owned(),
            reason: "invalid Azure DevOps organization slug".to_owned(),
        });
    }
    let auth = encode_azure_pat_basic_auth(token)?;
    Ok(HttpRequestSpec {
        method: Method::GET,
        url: format!("https://dev.azure.com/{org}/_apis/connectionData?api-version=7.1-preview"),
        headers: vec![
            ("authorization".to_owned(), auth),
            ("accept".to_owned(), "application/json".to_owned()),
        ],
        body: None,
    })
}

/// Builds an `az devops invoke --area Location --resource connectionData` command verifying auth (`#24170`, `da60db38e8`).
pub fn build_azure_connection_data_cli_spec(
    launch: &AzureCliLaunch,
    remote: &ParsedRemote,
    pat: Option<&str>,
) -> Result<CliCommandSpec, ForgeError> {
    let mut args = launch.prefix_args.clone();
    let org = remote
        .azure
        .as_ref()
        .map_or(remote.owner.as_str(), |az| az.organization.as_str());
    args.extend([
        "devops".to_owned(),
        "invoke".to_owned(),
        "--area".to_owned(),
        "Location".to_owned(),
        "--resource".to_owned(),
        "connectionData".to_owned(),
        "--api-version".to_owned(),
        "7.1-preview".to_owned(),
        "--organization".to_owned(),
        format!("https://dev.azure.com/{org}"),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    let env = match pat {
        Some(token) => azure_pat_cli_env(launch, token)?,
        None => launch.env.clone(),
    };
    Ok(CliCommandSpec {
        program: launch.program.clone(),
        args,
        env,
        stdin: None,
    })
}

/// Builds an `az devops project list --top 1` fallback probe spec (`#33864`).
pub fn build_azure_project_list_cli_spec(
    launch: &AzureCliLaunch,
    organization: &str,
    pat: Option<&str>,
) -> Result<CliCommandSpec, ForgeError> {
    let org = organization.trim().trim_matches('/');
    if org.is_empty() || org.contains(['/', '\\', '?', '#']) {
        return Err(ForgeError::InvalidHost {
            host: organization.to_owned(),
            reason: "invalid Azure DevOps organization slug".to_owned(),
        });
    }
    let mut args = launch.prefix_args.clone();
    args.extend([
        "devops".to_owned(),
        "project".to_owned(),
        "list".to_owned(),
        "--top".to_owned(),
        "1".to_owned(),
        "--organization".to_owned(),
        format!("https://dev.azure.com/{org}"),
        "--output".to_owned(),
        "json".to_owned(),
    ]);
    let env = match pat {
        Some(token) => azure_pat_cli_env(launch, token)?,
        None => launch.env.clone(),
    };
    Ok(CliCommandSpec {
        program: launch.program.clone(),
        args,
        env,
        stdin: None,
    })
}

/// Validates an `Azure` `DevOps` `connectionData` payload and rejects unauthenticated/anonymous
/// (`System:PublicAccess`) viewers (`#24170`, `da60db38e8`).
pub fn verify_azure_connection_data(
    body_json: &Value,
) -> Result<AzureAuthenticatedViewer, ForgeError> {
    let Some(user) = body_json.get("authenticatedUser").filter(|v| v.is_object()) else {
        return Err(ForgeError::AuthRequired(
            "Azure DevOps connectionData omitted authenticatedUser".to_owned(),
        ));
    };
    let id = json_str(user, "id").unwrap_or("").trim().to_owned();
    let display_name = json_str(user, "customDisplayName")
        .or_else(|| json_str(user, "providerDisplayName"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let subject_desc = json_str(user, "subjectDescriptor").unwrap_or("");
    let is_anonymous = id.is_empty()
        || subject_desc.eq_ignore_ascii_case("System:PublicAccess")
        || display_name
            .as_deref()
            .is_some_and(|d| d.eq_ignore_ascii_case("Anonymous"));
    if is_anonymous {
        return Err(ForgeError::AuthRequired(
            "Azure DevOps token resolved to an anonymous/public identity".to_owned(),
        ));
    }
    Ok(AzureAuthenticatedViewer { id, display_name })
}

fn validate_azure_pat(pat: &str) -> Result<&str, ForgeError> {
    let trimmed = pat.trim();
    if trimmed.is_empty() || trimmed.contains(['\r', '\n', '\0']) {
        return Err(ForgeError::InvalidCredentials(
            "Azure DevOps token is empty or contains line breaks".to_owned(),
        ));
    }
    Ok(trimmed)
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = usize::from(chunk[0]);
        let b1 = usize::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = usize::from(chunk.get(2).copied().unwrap_or(0));
        out.push(char::from(TABLE[b0 >> 2]));
        out.push(char::from(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)]));
        out.push(if chunk.len() > 1 {
            char::from(TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)])
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            char::from(TABLE[b2 & 0x3f])
        } else {
            '='
        });
    }
    out
}

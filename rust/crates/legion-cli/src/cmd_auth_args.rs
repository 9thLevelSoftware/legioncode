//! Argument parsing helpers for `legioncode auth pairing|session|connect-device`.

use legion_persistence::auth_mutations::parse_cli_duration_ms;

use crate::cli_global::fail_with_command_help;
use crate::fallback::take_flag_value;

#[derive(Default)]
pub(crate) struct AuthCommonFlags {
    pub(crate) base_dir: Option<String>,
    pub(crate) dev_url: Option<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) ttl_ms: Option<u64>,
    pub(crate) label: Option<String>,
    pub(crate) subject: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) output_mode: AuthOutputMode,
    pub(crate) positionals: Vec<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum AuthOutputMode {
    #[default]
    Human,
    Json,
    TokenOnly,
}

#[derive(Clone, Copy)]
pub(crate) struct AllowFlags(u8);

impl AllowFlags {
    const JSON: u8 = 1;
    const SCOPES: u8 = 2;
    const TTL: u8 = 4;
    const LABEL: u8 = 8;
    const SUBJECT: u8 = 16;
    const BASE_URL: u8 = 32;
    const TOKEN_ONLY: u8 = 64;

    pub(crate) const fn location_only() -> Self {
        Self(0)
    }

    pub(crate) const fn list_only() -> Self {
        Self(Self::JSON)
    }

    pub(crate) const fn pairing_create() -> Self {
        Self(Self::JSON | Self::SCOPES | Self::TTL | Self::LABEL | Self::BASE_URL)
    }

    pub(crate) const fn session_issue() -> Self {
        Self(Self::JSON | Self::SCOPES | Self::TTL | Self::LABEL | Self::SUBJECT | Self::TOKEN_ONLY)
    }

    const fn has(self, bit: u8) -> bool {
        (self.0 & bit) != 0
    }
}

pub(crate) fn parse_auth_flags(
    path: &[&str],
    tail: &[String],
    allow: AllowFlags,
) -> Result<AuthCommonFlags, i32> {
    let mut out = AuthCommonFlags::default();
    let mut idx = 0;
    let mut end_of_opts = false;

    while idx < tail.len() {
        let arg = &tail[idx];
        if !end_of_opts && arg == "--" {
            end_of_opts = true;
            idx += 1;
            continue;
        }
        if end_of_opts || !arg.starts_with('-') {
            out.positionals.push(arg.clone());
            idx += 1;
            continue;
        }
        if allow.has(AllowFlags::JSON) && arg == "--json" {
            if out.output_mode != AuthOutputMode::TokenOnly {
                out.output_mode = AuthOutputMode::Json;
            }
        } else if allow.has(AllowFlags::TOKEN_ONLY) && arg == "--token-only" {
            out.output_mode = AuthOutputMode::TokenOnly;
        } else if let Some(val) = take_opt(path, tail, &mut idx, "base-dir")? {
            out.base_dir = Some(val);
        } else if let Some(val) = take_opt(path, tail, &mut idx, "dev-url")? {
            validate_http_url(path, "--dev-url", &val)?;
            out.dev_url = Some(val);
        } else if allow.has(AllowFlags::SCOPES)
            && let Some(val) = take_opt(path, tail, &mut idx, "scopes")?
        {
            for item in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                out.scopes.push(item.to_owned());
            }
        } else if allow.has(AllowFlags::TTL)
            && let Some(val) = take_opt(path, tail, &mut idx, "ttl")?
        {
            let ms =
                parse_cli_duration_ms(&val).map_err(|msg| fail_with_command_help(path, &msg))?;
            out.ttl_ms = Some(ms);
        } else if allow.has(AllowFlags::LABEL)
            && let Some(val) = take_opt(path, tail, &mut idx, "label")?
        {
            out.label = Some(val);
        } else if allow.has(AllowFlags::SUBJECT)
            && let Some(val) = take_opt(path, tail, &mut idx, "subject")?
        {
            out.subject = Some(val);
        } else if allow.has(AllowFlags::BASE_URL)
            && let Some(val) = take_opt(path, tail, &mut idx, "base-url")?
        {
            validate_http_url(path, "--base-url", &val)?;
            out.base_url = Some(val);
        } else {
            return Err(fail_with_command_help(
                path,
                &format!(
                    "Unrecognized flag: {arg} in command legioncode {}",
                    path.join(" ")
                ),
            ));
        }
        idx += 1;
    }
    Ok(out)
}

fn validate_http_url(path: &[&str], flag: &str, val: &str) -> Result<(), i32> {
    if val.starts_with("http://") || val.starts_with("https://") {
        Ok(())
    } else {
        Err(fail_with_command_help(
            path,
            &format!(
                "Invalid value for flag {flag}: \"{val}\". Expected an http:// or https:// URL."
            ),
        ))
    }
}

fn take_opt(
    path: &[&str],
    tail: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    take_flag_value(tail, idx, name)
        .map_err(|()| fail_with_command_help(path, &format!("Missing value for flag --{name}")))
}

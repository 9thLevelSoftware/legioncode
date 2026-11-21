//! Native `t3 connect` entrypoint and argument parser backed by `cmd_connect_ops`.

use std::env;
use std::io::{self, Write};
use std::time::Duration;

use serde_json::Value;
use t3_remote::live_network::{CloudConnectManager, DeviceAuthorizationConfig};

use crate::cli_global::fail_with_command_help;
pub(crate) use crate::cmd_connect_ops::read_cloud_status_flag_pair;
use crate::cmd_connect_ops::{exec_disconnect, exec_link, exec_login, exec_publish, exec_status};
use crate::fallback::{delegate_to_node, take_flag_value, unit_forced_original, write_stdout_raw};
use crate::invocation::{has_cloud_public_config, resolve_host_cli_command};

/// Unit identifier for `RUST_ANYTHING_UNIT_CONNECT` and fallback logs.
pub const UNIT_CONNECT: &str = "CONNECT";

const CONNECT_UNAVAILABLE_STDOUT: &str = include_str!("../assets/help/connect.txt");
const CONNECT_UNAVAILABLE_STDERR: &str = "\nERROR\n  T3 Connect commands are unavailable: this build is missing T3 Connect public configuration.\n";

/// Runs `t3 connect`, printing unavailable diagnostics when unconfigured, or dispatching subcommands natively.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_CONNECT) {
        return delegate_to_node(UNIT_CONNECT, args);
    }

    if !has_cloud_public_config() {
        let _ = write_stdout_raw(CONNECT_UNAVAILABLE_STDOUT, "connect unavailable");
        let mut err = io::stderr().lock();
        let _ = err.write_all(CONNECT_UNAVAILABLE_STDERR.as_bytes());
        let _ = err.flush();
        return 1;
    }

    let rest = args.get(1..).unwrap_or(&[]);
    let (sub, tail) = match rest.first().map(String::as_str) {
        Some(s) if !s.starts_with('-') => (Some(s), &rest[1..]),
        _ => (None, rest),
    };

    match sub {
        None => match parse_connect_flags(&["connect"], tail, FlagMask::HEADLESS) {
            Ok(f) => exec_link(f.base_dir.as_deref(), f.has(FlagMask::HEADLESS), false),
            Err(code) => code,
        },
        Some("link") => match parse_connect_flags(&["connect", "link"], tail, FlagMask::LINK) {
            Ok(f) => exec_link(
                f.base_dir.as_deref(),
                f.has(FlagMask::HEADLESS),
                f.has(FlagMask::PUBLISH_ONLY),
            ),
            Err(code) => code,
        },
        Some("login") => match parse_connect_flags(&["connect", "login"], tail, FlagMask::HEADLESS)
        {
            Ok(f) => exec_login(f.base_dir.as_deref(), f.has(FlagMask::HEADLESS)),
            Err(code) => code,
        },
        Some("status") => match parse_connect_flags(&["connect", "status"], tail, FlagMask::JSON) {
            Ok(f) => {
                let link_args = f
                    .base_dir
                    .as_deref()
                    .map(|d| vec!["--base-dir".to_owned(), d.to_owned()])
                    .unwrap_or_default();
                let link_cmd = resolve_host_cli_command("connect link", &link_args);
                exec_status(f.base_dir.as_deref(), f.has(FlagMask::JSON), &link_cmd)
            }
            Err(code) => code,
        },
        Some("publish") => {
            match parse_connect_flags(&["connect", "publish"], tail, FlagMask::DISABLE) {
                Ok(f) => exec_publish(f.base_dir.as_deref(), f.has(FlagMask::DISABLE)),
                Err(code) => code,
            }
        }
        Some(s @ ("unlink" | "disconnect")) => {
            match parse_connect_flags(&["connect", s], tail, FlagMask::NONE) {
                Ok(f) => exec_disconnect(f.base_dir.as_deref(), false),
                Err(code) => code,
            }
        }
        Some("logout") => match parse_connect_flags(&["connect", "logout"], tail, FlagMask::NONE) {
            Ok(f) => exec_disconnect(f.base_dir.as_deref(), true),
            Err(code) => code,
        },
        Some(other) => fail_with_command_help(
            &["connect"],
            &format!("Unknown subcommand \"{other}\" for \"t3 connect\""),
        ),
    }
}

#[derive(Clone, Copy)]
struct FlagMask(u8);

impl FlagMask {
    const NONE: Self = Self(0);
    const HEADLESS: Self = Self(1);
    const JSON: Self = Self(2);
    const PUBLISH_ONLY: Self = Self(4);
    const DISABLE: Self = Self(8);
    const LINK: Self = Self(Self::HEADLESS.0 | Self::PUBLISH_ONLY.0);

    const fn has(self, bit: Self) -> bool {
        (self.0 & bit.0) != 0
    }
}

struct CommonConnectFlags {
    base_dir: Option<String>,
    bits: u8,
}

impl CommonConnectFlags {
    const fn has(&self, mask: FlagMask) -> bool {
        (self.bits & mask.0) != 0
    }
}

fn parse_connect_flags(
    path: &[&str],
    tail: &[String],
    mask: FlagMask,
) -> Result<CommonConnectFlags, i32> {
    let mut flags = CommonConnectFlags {
        base_dir: None,
        bits: 0,
    };
    let mut idx = 0;
    while idx < tail.len() {
        let arg = &tail[idx];
        if mask.has(FlagMask::HEADLESS) && arg == "--headless" {
            flags.bits |= FlagMask::HEADLESS.0;
        } else if mask.has(FlagMask::JSON) && arg == "--json" {
            flags.bits |= FlagMask::JSON.0;
        } else if mask.has(FlagMask::PUBLISH_ONLY) && arg == "--publish-only" {
            flags.bits |= FlagMask::PUBLISH_ONLY.0;
        } else if mask.has(FlagMask::DISABLE) && arg == "--disable" {
            flags.bits |= FlagMask::DISABLE.0;
        } else {
            match take_flag_value(tail, &mut idx, "base-dir") {
                Ok(Some(val)) => flags.base_dir = Some(val),
                Ok(None) if arg.starts_with('-') => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unrecognized flag: {arg} in command t3 {}", path.join(" ")),
                    ));
                }
                Ok(None) => {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Unexpected argument: {arg}"),
                    ));
                }
                Err(()) => {
                    return Err(fail_with_command_help(
                        path,
                        "Missing value for flag --base-dir",
                    ));
                }
            }
        }
        idx += 1;
    }
    Ok(flags)
}

pub(crate) fn run_device_auth(mgr: &CloudConnectManager, _headless: bool) -> i32 {
    let Some(cfg) = resolve_device_auth_config() else {
        eprintln!("T3 Connect OAuth endpoints are not configured in environment.");
        return 1;
    };
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `t3 connect`.");
        return 1;
    };
    match rt.block_on(async {
        let prompt = mgr.start_device_authorization(&cfg).await?;
        let uri = prompt
            .verification_uri_complete
            .as_deref()
            .unwrap_or(&prompt.verification_uri);
        println!(
            "Sign in to T3 Connect. Open on any device:\n  {uri}\nCode: {}",
            prompt.user_code
        );
        mgr.poll_device_token(
            &cfg,
            &prompt.device_code,
            Duration::from_secs(prompt.interval.unwrap_or(5).max(1)),
            Duration::from_secs(prompt.expires_in.max(1)),
        )
        .await
    }) {
        Ok(_) => 0,
        Err(err) => {
            eprintln!("T3 Connect authorization failed: {err}");
            1
        }
    }
}

fn resolve_device_auth_config() -> Option<DeviceAuthorizationConfig> {
    let raw = env::var("T3CODE_CLOUD_PUBLIC_CONFIG").ok()?;
    let val: Value = serde_json::from_str(&raw).ok()?;
    Some(DeviceAuthorizationConfig {
        device_authorization_endpoint: val.get("deviceAuthorizationEndpoint")?.as_str()?.to_owned(),
        token_endpoint: val.get("tokenEndpoint")?.as_str()?.to_owned(),
        client_id: val.get("clientId")?.as_str()?.to_owned(),
        scopes: vec![
            "openid".to_owned(),
            "profile".to_owned(),
            "offline_access".to_owned(),
        ],
        request_timeout: Duration::from_secs(10),
        slow_down_increment: Duration::from_secs(5),
    })
}

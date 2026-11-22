//! Standalone `legioncode pair` QR pairing command and shared local pairing session helper.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use legion_contracts::environment::{
    ExecutionEnvironmentDescriptor, ORCHESTRATION_PROTOCOL_VERSION,
};
use legion_persistence::ownership::PersistedServerRuntimeState;
use legion_persistence::pairing_tokens::{
    DEFAULT_PAIRING_TOKEN_TTL_MS, IssuePairingTokenInput, STANDARD_CLIENT_SCOPES,
    issue_pairing_token,
};
use legion_shared::qr_code::{build_pairing_app_link, build_pairing_url, render_terminal_qr_code};
use serde::Deserialize;
use serde_json::json;

use crate::cli_global::fail_with_command_help;
use crate::cmd_pair_ts::{
    DEFAULT_SERVE_PORT, PairOutputFields, discover_pair_target, format_pair_output,
    publish_tailscale_pairing_base, render_pair_json, resolve_direct_pairing_base,
};
use crate::cmd_trace::{current_epoch_ms, parse_duration_ms};
use crate::fallback::{
    delegate_to_node, resolve_base_dir, take_flag_value, unit_forced_original, write_stdout_line,
};

/// Unit identifier for `RUST_ANYTHING_UNIT_PAIR` and fallback logs.
pub const UNIT_PAIR: &str = "PAIR";

#[derive(Debug, Clone)]
struct PairFlags {
    base_dir: Option<String>,
    ttl_ms: i64,
    label: Option<String>,
    scopes: Vec<String>,
    tailscale: bool,
    tailscale_serve_port: u16,
    json: bool,
}

/// Runs `legioncode pair` natively (including `--tailscale` and `--json`) unless `RUST_ANYTHING_UNIT_PAIR=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_PAIR) {
        return delegate_to_node(UNIT_PAIR, args);
    }

    let rest = args.get(1..).unwrap_or(&[]);
    let flags = match parse_pair_flags(rest) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `legioncode pair`.");
        return 1;
    };
    rt.block_on(execute_pair(&flags))
}

async fn execute_pair(flags: &PairFlags) -> i32 {
    let (base_dir, _) = resolve_base_dir(flags.base_dir.as_deref());
    let Some((variant, db_path, runtime, descriptor)) = discover_pair_target(&base_dir).await
    else {
        eprintln!(
            "No running LegionCode server found under {}. Start LegionCode (`legioncode serve`), then retry.",
            base_dir.display()
        );
        return 1;
    };

    let server_proto = descriptor.orchestration_protocol_version.unwrap_or(1);
    if server_proto != ORCHESTRATION_PROTOCOL_VERSION {
        eprintln!(
            "Running LegionCode server {} uses orchestration protocol v{server_proto}, which is incompatible with protocol v{ORCHESTRATION_PROTOCOL_VERSION}.",
            descriptor.server_version
        );
        return 1;
    }

    let input = IssuePairingTokenInput {
        scopes: flags.scopes.clone(),
        ttl_ms: flags.ttl_ms,
        label: Some(
            flags
                .label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("legioncode pair")
                .to_owned(),
        ),
    };
    let issued = match issue_pairing_token(&db_path, &input, current_epoch_ms()) {
        Ok(issued) => issued,
        Err(err) => {
            eprintln!("Failed to mint pairing credential: {err}");
            return 1;
        }
    };

    let mut notes = Vec::new();
    let pairing_base_url = if flags.tailscale {
        match publish_tailscale_pairing_base(&runtime, &descriptor, flags.tailscale_serve_port)
            .await
        {
            Ok((base_url, note)) => {
                notes.push(note);
                base_url
            }
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
    } else {
        let (direct_url, is_loopback) = resolve_direct_pairing_base(&runtime);
        if is_loopback {
            notes.push(
                "This URL is only reachable from this machine. To reach it from your other devices, run `legioncode up`, or re-run with --tailscale.".to_owned(),
            );
        } else if runtime.private_network_only == Some(true) {
            notes.push(
                "This URL works only from this machine's local network. From elsewhere, re-run with --tailscale or use Legion Connect.".to_owned(),
            );
        }
        if variant == "dev" && runtime.dev_url.is_none() {
            notes.push(
                "This dev server did not record its web URL; restart it so pairing can go through the web origin.".to_owned(),
            );
        }
        direct_url
    };

    emit_pair_result(
        flags,
        &runtime,
        &descriptor,
        &issued,
        &pairing_base_url,
        &notes,
    )
}

fn emit_pair_result(
    flags: &PairFlags,
    runtime: &PersistedServerRuntimeState,
    descriptor: &ExecutionEnvironmentDescriptor,
    issued: &legion_persistence::pairing_tokens::IssuedPairingToken,
    pairing_base_url: &str,
    notes: &[String],
) -> i32 {
    let Ok(pairing_url) = build_pairing_url(pairing_base_url, &issued.credential) else {
        eprintln!("Failed to build pairing URL from `{pairing_base_url}`.");
        return 1;
    };
    let app_link = build_pairing_app_link(&pairing_url);

    if flags.json {
        return match render_pair_json(
            issued,
            &runtime.origin,
            &pairing_url,
            &app_link,
            &flags.scopes,
        ) {
            Ok(rendered) => write_stdout_line(&rendered, "pair"),
            Err(err) => {
                eprintln!("{err}");
                1
            }
        };
    }

    let Ok(qr) = render_terminal_qr_code(&pairing_url, 2) else {
        eprintln!("Failed to render terminal QR code for pairing URL.");
        return 1;
    };
    let note_refs: Vec<&str> = notes.iter().map(String::as_str).collect();
    let rendered = format_pair_output(&PairOutputFields {
        server_label: &descriptor.label,
        origin: &runtime.origin,
        qr: &qr,
        pairing_url: &pairing_url,
        app_link: &app_link,
        token: &issued.credential,
        expires_at_iso: &issued.expires_at_iso,
        notes: &note_refs,
    });
    write_stdout_line(&rendered, "pair")
}

/// Mints a short-lived pairing token into `db_path` and exchanges it via `POST /api/auth/pair`.
pub(crate) async fn exchange_local_bearer(
    db_path: &Path,
    origin: &str,
    client_label: &str,
) -> Result<String, String> {
    let now_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let input = IssuePairingTokenInput::default()
        .with_ttl_ms(60_000)
        .with_label(client_label);
    let issued = issue_pairing_token(db_path, &input, now_ms)
        .map_err(|err| format!("Failed to mint CLI pairing token: {err}"))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| format!("Failed to build HTTP client: {err}"))?;
    let pair_url = format!("{}/api/auth/pair", origin.trim_end_matches('/'));
    let resp = client
        .post(&pair_url)
        .json(&json!({ "token": issued.credential, "clientLabel": client_label }))
        .send()
        .await
        .map_err(|err| format!("Could not reach the LegionCode server at {origin}: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Pairing exchange against {origin} failed with HTTP {}.",
            resp.status().as_u16()
        ));
    }

    let body: PairReply = resp
        .json()
        .await
        .map_err(|err| format!("Invalid /api/auth/pair response from {origin}: {err}"))?;
    Ok(body.access_token)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairReply {
    access_token: String,
}

fn parse_pair_flags(rest: &[String]) -> Result<PairFlags, i32> {
    let path = &["pair"];
    let mut base_dir = None;
    let mut ttl_ms = DEFAULT_PAIRING_TOKEN_TTL_MS;
    let mut label = None;
    let mut scopes = Vec::new();
    let mut tailscale = false;
    let mut tailscale_serve_port = DEFAULT_SERVE_PORT;
    let mut json = false;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if arg == "--tailscale" {
            tailscale = true;
        } else if arg == "--json" {
            json = true;
        } else if let Some(val) = take_pair_opt(path, rest, &mut idx, "base-dir")? {
            base_dir = Some(val);
        } else if let Some(val) = take_pair_opt(path, rest, &mut idx, "ttl")? {
            ttl_ms = parse_duration_ms(&val).ok_or_else(|| {
                fail_with_command_help(
                    path,
                    "Invalid duration. Use values like 5m, 1h, 30d, or 15 minutes.",
                )
            })?;
        } else if let Some(val) = take_pair_opt(path, rest, &mut idx, "label")? {
            label = Some(val);
        } else if let Some(val) = take_pair_opt(path, rest, &mut idx, "tailscale-serve-port")? {
            tailscale_serve_port = val.trim().parse::<u16>().map_err(|_| {
                fail_with_command_help(
                    path,
                    &format!("Invalid value for flag --tailscale-serve-port: \"{val}\"."),
                )
            })?;
        } else if let Some(val) = take_pair_opt(path, rest, &mut idx, "scope")?
            .or(take_pair_opt(path, rest, &mut idx, "scopes")?)
        {
            for s in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                scopes.push(s.to_owned());
            }
        } else if arg.starts_with('-') {
            return Err(fail_with_command_help(
                path,
                &format!("Unrecognized flag: {arg} in command legioncode pair"),
            ));
        } else {
            return Err(fail_with_command_help(
                path,
                &format!("Unexpected argument: {arg}"),
            ));
        }
        idx += 1;
    }

    if scopes.is_empty() {
        scopes = STANDARD_CLIENT_SCOPES
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
    }
    Ok(PairFlags {
        base_dir,
        ttl_ms,
        label,
        scopes,
        tailscale,
        tailscale_serve_port,
        json,
    })
}

fn take_pair_opt(
    path: &[&str],
    rest: &[String],
    idx: &mut usize,
    name: &str,
) -> Result<Option<String>, i32> {
    take_flag_value(rest, idx, name)
        .map_err(|()| fail_with_command_help(path, &format!("Missing value for flag --{name}")))
}

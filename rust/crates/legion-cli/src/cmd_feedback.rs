#![forbid(unsafe_code)]
//! Standalone `legioncode feedback` CLI subcommands (`ADR-013` local-only `sent = 0`, `#35871`).

use std::path::{Path, PathBuf};

use legion_contracts::brand::ACTIVE_BRAND;
use legion_remote::acp_mcp_bridge::McpHttpClient;
use legion_remote::thread_client::discover_live_server_runtime;
use legion_workflows::trace_types::FeedbackKind;
use serde_json::{Value, json};

use crate::cli_global::fail_with_command_help;
use crate::cmd_thread::ops::{
    acquire_local_thread_auth, extract_mcp_tool_structured, print_json_value,
    revoke_fallback_thread_session,
};
use crate::fallback::{delegate_to_node, resolve_base_dir, unit_forced_original};

#[path = "cmd_feedback_list.rs"]
mod list;
use list::run_list;

/// Unit identifier for `RUST_ANYTHING_UNIT_FEEDBACK` fallback control.
pub const UNIT_FEEDBACK: &str = "FEEDBACK";

#[derive(Debug, Clone)]
enum FeedbackCliAction {
    List {
        base_dir: Option<PathBuf>,
        json: bool,
    },
    Submit {
        base_dir: Option<PathBuf>,
        remote_url: Option<String>,
        token: Option<String>,
        kind: FeedbackKind,
        subject: String,
        expected: String,
        actual: String,
        workaround: String,
    },
}

/// Runs `legioncode feedback ...` storing reports locally only (`sent = 0`).
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_FEEDBACK) {
        return delegate_to_node(UNIT_FEEDBACK, args);
    }
    let rest = args.get(1..).unwrap_or(&[]);
    let action = match parse_feedback_cli(rest) {
        Ok(a) => a,
        Err(code) => return code,
    };
    match action {
        FeedbackCliAction::List { base_dir, json } => run_list(base_dir.as_deref(), json),
        FeedbackCliAction::Submit {
            base_dir,
            remote_url,
            token,
            kind,
            subject,
            expected,
            actual,
            workaround,
        } => {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                eprintln!("Failed to initialize async runtime for `legioncode feedback`.");
                return 1;
            };
            rt.block_on(run_submit(
                base_dir.as_deref(),
                remote_url,
                token,
                kind,
                subject,
                expected,
                actual,
                workaround,
            ))
        }
    }
}

fn parse_feedback_cli(args: &[String]) -> Result<FeedbackCliAction, i32> {
    if args.first().map(String::as_str) == Some("list") {
        let path = &["feedback", "list"];
        let mut base_dir = None;
        let mut json = false;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--json" {
                json = true;
            } else if args[i] == "--base-dir" {
                i += 1;
                let Some(val) = args.get(i) else {
                    return Err(fail_with_command_help(
                        path,
                        "Missing value for --base-dir.",
                    ));
                };
                base_dir = Some(PathBuf::from(val));
            } else if let Some(val) = args[i].strip_prefix("--base-dir=") {
                base_dir = Some(PathBuf::from(val));
            } else {
                return Err(fail_with_command_help(
                    path,
                    &format!("Unknown flag `{}`.", args[i]),
                ));
            }
            i += 1;
        }
        return Ok(FeedbackCliAction::List { base_dir, json });
    }

    let path = &["feedback"];
    let mut base_dir = None;
    let mut remote_url = None;
    let mut token = None;
    let mut kind: Option<FeedbackKind> = None;
    let mut subject: Option<String> = None;
    let mut expected: Option<String> = None;
    let mut actual: Option<String> = None;
    let mut workaround = String::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let take_val = |idx: &mut usize, name: &str| -> Result<Option<String>, i32> {
            let flag = format!("--{name}");
            if args[*idx] == flag {
                *idx += 1;
                let Some(v) = args.get(*idx) else {
                    return Err(fail_with_command_help(
                        path,
                        &format!("Missing value for --{name}."),
                    ));
                };
                return Ok(Some(v.clone()));
            }
            Ok(args[*idx]
                .strip_prefix(&format!("{flag}="))
                .map(str::to_owned))
        };
        if let Some(v) = take_val(&mut i, "base-dir")? {
            base_dir = Some(PathBuf::from(v));
        } else if let Some(v) = take_val(&mut i, "remote-url")? {
            remote_url = Some(v);
        } else if let Some(v) = take_val(&mut i, "token")? {
            token = Some(v);
        } else if let Some(v) = take_val(&mut i, "kind")? {
            let parsed = serde_json::from_value::<FeedbackKind>(Value::String(v.clone()))
                .map_err(|_| fail_with_command_help(path, &format!("Invalid --kind `{v}`.")))?;
            kind = Some(parsed);
        } else if let Some(v) = take_val(&mut i, "subject")? {
            subject = Some(v);
        } else if let Some(v) = take_val(&mut i, "expected")? {
            expected = Some(v);
        } else if let Some(v) = take_val(&mut i, "actual")? {
            actual = Some(v);
        } else if let Some(v) = take_val(&mut i, "workaround")? {
            workaround = v;
        } else {
            return Err(fail_with_command_help(
                path,
                &format!("Unknown argument `{arg}`."),
            ));
        }
        i += 1;
    }

    Ok(FeedbackCliAction::Submit {
        base_dir,
        remote_url,
        token,
        kind: kind.ok_or_else(|| fail_with_command_help(path, "--kind is required."))?,
        subject: subject.ok_or_else(|| fail_with_command_help(path, "--subject is required."))?,
        expected: expected
            .ok_or_else(|| fail_with_command_help(path, "--expected is required."))?,
        actual: actual.ok_or_else(|| fail_with_command_help(path, "--actual is required."))?,
        workaround,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Direct CLI flag container for feedback submission."
)]
async fn run_submit(
    base_dir_arg: Option<&Path>,
    remote_url: Option<String>,
    token: Option<String>,
    kind: FeedbackKind,
    subject: String,
    expected: String,
    actual: String,
    workaround: String,
) -> i32 {
    let (origin, endpoint, bearer, local_session) = if let Some(remote) = remote_url {
        let base = remote.trim_end_matches('/').to_owned();
        let Some(tok) = token.or_else(|| ACTIVE_BRAND.read_env("TOKEN")) else {
            eprintln!("--remote-url requires --token.");
            return 1;
        };
        (base.clone(), format!("{base}/mcp"), tok, None)
    } else {
        let (base_dir, _) = resolve_base_dir(base_dir_arg.and_then(Path::to_str));
        let discovered = match discover_live_server_runtime(&base_dir) {
            Ok(d) => d,
            Err(err) => {
                eprintln!("{err}");
                return err.exit_code();
            }
        };
        let db_path = discovered.state_dir.join("statev2.sqlite");
        let (b, sid) = match acquire_local_thread_auth(&db_path, &discovered.runtime.origin).await {
            Ok(pair) => pair,
            Err(msg) => {
                eprintln!("{msg}");
                return 1;
            }
        };
        (
            discovered.runtime.origin,
            discovered.mcp_endpoint,
            b,
            sid.map(|id| (db_path, id)),
        )
    };
    let Ok(mcp) = McpHttpClient::connect(
        &endpoint,
        format!("Bearer {bearer}"),
        "legioncode-feedback-cli",
    )
    .await
    else {
        revoke_fallback_thread_session(local_session.as_ref());
        eprintln!("Could not reach the LegionCode server at {origin}.");
        return 1;
    };
    let tool = format!("{}feedback", ACTIVE_BRAND.mcp_tool_prefix);
    let args = json!({
        "kind": kind,
        "subject": subject,
        "expected": expected,
        "actual": actual,
        "workaround": workaround,
    });
    let res = mcp.call_tool(&tool, &args).await;
    revoke_fallback_thread_session(local_session.as_ref());
    match res {
        Ok(raw) => match extract_mcp_tool_structured(raw) {
            Ok(structured) => print_json_value(&structured),
            Err(err) => {
                eprintln!("{err}");
                1
            }
        },
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

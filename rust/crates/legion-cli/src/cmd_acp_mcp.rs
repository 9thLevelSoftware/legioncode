//! Standalone `legioncode acp-mcp-bridge` and `legioncode acp-mcp-call` backed by `legion_remote::acp_mcp_bridge`.

use std::env;
use std::io::{self, Write};

use legion_remote::acp_mcp_bridge::{McpBridgeError, call_mcp_tool, run_mcp_stdio_bridge};
use tokio::io::BufReader;

use crate::cli_global::fail_cli_error;
use crate::fallback::{delegate_to_node, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_ACP_MCP` and fallback logs.
pub const UNIT_ACP_MCP: &str = "ACP_MCP";

/// Runs `legioncode acp-mcp-bridge` or `legioncode acp-mcp-call <tool> <arguments-json>` natively.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_ACP_MCP) {
        return delegate_to_node(UNIT_ACP_MCP, args);
    }

    let Some(cmd) = args.first().map(String::as_str) else {
        return fail_cli_error("Missing ACP MCP command.");
    };

    let Some((endpoint, authorization)) = resolve_mcp_env() else {
        eprintln!(
            "{cmd} requires LEGIONCODE_ACP_MCP_ENDPOINT and LEGIONCODE_ACP_MCP_AUTHORIZATION."
        );
        return 2;
    };

    let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        eprintln!("Failed to initialize async runtime for `{cmd}`.");
        return 1;
    };

    match cmd {
        "acp-mcp-bridge" => {
            if let Some(extra) = args.get(1) {
                return fail_cli_error(&format!(
                    "Unexpected argument `{extra}` for `acp-mcp-bridge`."
                ));
            }
            let res = rt.block_on(run_mcp_stdio_bridge(
                &endpoint,
                &authorization,
                BufReader::new(tokio::io::stdin()),
                tokio::io::stdout(),
            ));
            match res {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("{err}");
                    1
                }
            }
        }
        "acp-mcp-call" => run_call(&rt, &endpoint, &authorization, args),
        other => fail_cli_error(&format!("Unknown ACP MCP command `{other}`.")),
    }
}

fn run_call(
    rt: &tokio::runtime::Runtime,
    endpoint: &str,
    authorization: &str,
    args: &[String],
) -> i32 {
    if args.len() != 3 {
        eprintln!("acp-mcp-call requires <tool> and <arguments-json>.");
        return 2;
    }
    let tool = &args[1];
    let args_json = &args[2];
    match rt.block_on(call_mcp_tool(endpoint, authorization, tool, args_json)) {
        Ok(value) => match serde_json::to_string(&value) {
            Ok(encoded) => {
                let mut out = io::stdout().lock();
                if writeln!(out, "{encoded}").is_err() || out.flush().is_err() {
                    return 1;
                }
                0
            }
            Err(err) => {
                eprintln!("{err}");
                1
            }
        },
        Err(McpBridgeError::InvalidInput { detail }) => {
            eprintln!("{detail}");
            2
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn resolve_mcp_env() -> Option<(String, String)> {
    let endpoint =
        first_non_empty_env(&["LEGIONCODE_ACP_MCP_ENDPOINT", "LEGIONCODE_MCP_ENDPOINT"])?;
    let authorization = first_non_empty_env(&[
        "LEGIONCODE_ACP_MCP_AUTHORIZATION",
        "LEGIONCODE_MCP_AUTHORIZATION",
    ])?;
    Some((endpoint, authorization))
}

fn first_non_empty_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|val| val.trim().to_owned())
            .filter(|val| !val.is_empty())
    })
}

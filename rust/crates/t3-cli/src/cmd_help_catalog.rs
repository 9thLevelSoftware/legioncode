//! Static `--help` / `-h` catalog matching `node apps/server/src/bin.ts <cmd...> --help`.

use crate::invocation::has_cloud_public_config;

/// Resolves exact Effect CLI `--help` output for a command token path (`["status"]`, etc.).
pub(crate) fn lookup_help(path: &[&str]) -> Option<&'static str> {
    let cloud_enabled = has_cloud_public_config();
    match path {
        [] => Some(if cloud_enabled {
            include_str!("../assets/help/enabled__root.txt")
        } else {
            include_str!("../assets/help/root.txt")
        }),
        ["help"] => Some(include_str!("../assets/help/help.txt")),
        ["start"] => Some(include_str!("../assets/help/start.txt")),
        ["serve"] => Some(include_str!("../assets/help/serve.txt")),
        ["app"] => Some(include_str!("../assets/help/app.txt")),
        ["pair"] => Some(include_str!("../assets/help/pair.txt")),
        ["up"] => Some(include_str!("../assets/help/up.txt")),
        ["status"] => Some(include_str!("../assets/help/status.txt")),
        ["update"] => Some(include_str!("../assets/help/update.txt")),
        ["install"] => Some(include_str!("../assets/help/install.txt")),
        ["uninstall"] => Some(include_str!("../assets/help/uninstall.txt")),
        ["doctor"] => Some(include_str!("../assets/help/doctor.txt")),
        ["triage"] => Some(include_str!("../assets/help/triage.txt")),
        ["__service-launcher"] => Some(include_str!("../assets/help/__service-launcher.txt")),
        ["__service-preflight"] => Some(include_str!("../assets/help/__service-preflight.txt")),
        ["__ssh-helper"] => Some(include_str!("../assets/help/__ssh-helper.txt")),
        _ => lookup_nested_help(path, cloud_enabled),
    }
}

fn lookup_nested_help(path: &[&str], cloud_enabled: bool) -> Option<&'static str> {
    match path {
        ["auth"] => Some(include_str!("../assets/help/auth.txt")),
        ["auth", "pairing"] => Some(include_str!("../assets/help/auth__pairing.txt")),
        ["auth", "pairing", "create"] => {
            Some(include_str!("../assets/help/auth__pairing__create.txt"))
        }
        ["auth", "pairing", "list"] => Some(include_str!("../assets/help/auth__pairing__list.txt")),
        ["auth", "pairing", "revoke"] => {
            Some(include_str!("../assets/help/auth__pairing__revoke.txt"))
        }
        ["auth", "session"] => Some(include_str!("../assets/help/auth__session.txt")),
        ["auth", "session", "issue"] => {
            Some(include_str!("../assets/help/auth__session__issue.txt"))
        }
        ["auth", "session", "list"] => Some(include_str!("../assets/help/auth__session__list.txt")),
        ["auth", "session", "revoke"] => {
            Some(include_str!("../assets/help/auth__session__revoke.txt"))
        }
        ["auth", "connect-device"] => Some(include_str!("../assets/help/auth__connect-device.txt")),
        ["auth", "connect-device", "list"] => Some(include_str!(
            "../assets/help/auth__connect-device__list.txt"
        )),
        ["auth", "connect-device", "approve"] => Some(include_str!(
            "../assets/help/auth__connect-device__approve.txt"
        )),
        ["auth", "connect-device", "revoke"] => Some(include_str!(
            "../assets/help/auth__connect-device__revoke.txt"
        )),
        ["environment"] => Some(include_str!("../assets/help/environment.txt")),
        ["environment", "link"] => Some(include_str!("../assets/help/environment__link.txt")),
        ["environment", "list"] => Some(include_str!("../assets/help/environment__list.txt")),
        ["environment", "unlink"] => Some(include_str!("../assets/help/environment__unlink.txt")),
        ["browser"] => Some(include_str!("../assets/help/browser.txt")),
        ["browser", "setup"] => Some(include_str!("../assets/help/browser__setup.txt")),
        ["project"] => Some(include_str!("../assets/help/project.txt")),
        ["project", "add"] => Some(include_str!("../assets/help/project__add.txt")),
        ["project", "remove"] => Some(include_str!("../assets/help/project__remove.txt")),
        ["project", "rename"] => Some(include_str!("../assets/help/project__rename.txt")),
        _ => lookup_nested_tail_help(path, cloud_enabled),
    }
}

fn lookup_nested_tail_help(path: &[&str], cloud_enabled: bool) -> Option<&'static str> {
    match path {
        ["thread"] | ["thread", "wait"] => Some(include_str!("../assets/help/thread.txt")),
        ["thread", "list"] => Some(include_str!("../assets/help/thread__list.txt")),
        ["thread", "start"] => Some(include_str!("../assets/help/thread__start.txt")),
        ["thread", "send"] => Some(include_str!("../assets/help/thread__send.txt")),
        ["thread", "show" | "read"] => Some(include_str!("../assets/help/thread__show.txt")),
        ["service"] => Some(include_str!("../assets/help/service.txt")),
        ["service", "install"] => Some(include_str!("../assets/help/service__install.txt")),
        ["service", "restart"] => Some(include_str!("../assets/help/service__restart.txt")),
        ["service", "uninstall"] => Some(include_str!("../assets/help/service__uninstall.txt")),
        ["service", "status"] => Some(include_str!("../assets/help/service__status.txt")),
        ["service", "protect"] => Some(include_str!("../assets/help/service__protect.txt")),
        ["theme"] => Some(include_str!("../assets/help/theme.txt")),
        ["theme", "clear"] => Some(include_str!("../assets/help/theme__clear.txt")),
        ["theme", "set"] => Some(include_str!("../assets/help/theme__set.txt")),
        ["theme", "show"] => Some(include_str!("../assets/help/theme__show.txt")),
        ["trace"] => Some(include_str!("../assets/help/trace.txt")),
        ["trace", "summary"] => Some(include_str!("../assets/help/trace__summary.txt")),
        ["policy"] => Some(include_str!("../assets/help/policy.txt")),
        ["policy", "show"] => Some(include_str!("../assets/help/policy__show.txt")),
        ["policy", "check"] => Some(include_str!("../assets/help/policy__check.txt")),
        ["policy", "templates"] => Some(include_str!("../assets/help/policy__templates.txt")),
        ["connect"] => Some(if cloud_enabled {
            include_str!("../assets/help/enabled__connect.txt")
        } else {
            include_str!("../assets/help/connect.txt")
        }),
        ["connect", sub] if cloud_enabled => match *sub {
            "login" => Some(include_str!("../assets/help/enabled__connect__login.txt")),
            "link" => Some(include_str!("../assets/help/enabled__connect__link.txt")),
            "publish" => Some(include_str!("../assets/help/enabled__connect__publish.txt")),
            "status" => Some(include_str!("../assets/help/enabled__connect__status.txt")),
            "unlink" => Some(include_str!("../assets/help/enabled__connect__unlink.txt")),
            "logout" => Some(include_str!("../assets/help/enabled__connect__logout.txt")),
            _ => None,
        },
        ["connect", _] => Some(include_str!("../assets/help/connect.txt")),
        ["acp-mcp-bridge"] => Some(
            "DESCRIPTION\n  Bridge T3 Code's MCP endpoint to stdio for ACP agents.\n\nUSAGE\n  t3 acp-mcp-bridge\n",
        ),
        ["acp-mcp-call"] => Some(
            "DESCRIPTION\n  Call one T3 Code MCP tool from an ACP agent terminal.\n\nUSAGE\n  t3 acp-mcp-call <tool> <arguments-json>\n",
        ),
        ["__speech-worker"] => Some(
            "DESCRIPTION\n  Internal T3 Code speech worker.\n\nUSAGE\n  t3 __speech-worker [--stdio | discover | model [path]]\n",
        ),
        ["__document-text"] => Some(
            "DESCRIPTION\n  Internal T3 Code document text extractor.\n\nUSAGE\n  t3 __document-text <path> <mime>\n",
        ),
        ["__managed-chatgpt-token"] => Some(
            "DESCRIPTION\n  Internal managed ChatGPT token reader.\n\nUSAGE\n  t3 __managed-chatgpt-token <endpoint>\n",
        ),
        ["__claude-history"] => Some(
            "DESCRIPTION\n  Internal Claude history worker.\n\nUSAGE\n  t3 __claude-history <method> <session-id> [options-json]\n",
        ),
        _ => None,
    }
}

/// Resolves the longest known command prefix from leading non-flag arguments.
pub(crate) fn resolve_command_path(args: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    for arg in args {
        if arg == "--" || arg.starts_with('-') {
            break;
        }
        words.push(arg.as_str());
        if words.len() >= 3 {
            break;
        }
    }
    while !words.is_empty() {
        if lookup_help(&words).is_some() {
            return words;
        }
        words.pop();
    }
    Vec::new()
}

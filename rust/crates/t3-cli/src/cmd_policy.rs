//! `t3 policy` subcommands (`show`, `check`, `settings`, `templates`) matching `apps/server/src/cli/policy.ts`.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use t3_contracts::brand::ACTIVE_BRAND;
use t3_contracts::policy::{
    LoadedPolicySummary, ManagedPolicyDocument, RunningServerState, describe_manageable_settings,
    describe_policy, describe_running_server, macos_profile, policy_fingerprint_12hex,
    sample_policy, windows_adml, windows_admx,
};
use t3_persistence::ownership::{ManagedPolicyState, read_live_server_runtime_state};
use uuid::Uuid;

use crate::cli_global::fail_with_command_help;
use crate::fallback::{
    delegate_to_node, derive_paths, take_flag_value, unit_forced_original, write_stdout_line,
};

/// Unit identifier for `RUST_ANYTHING_UNIT_POLICY` and fallback logs.
pub const UNIT_POLICY: &str = "POLICY";

const MANAGED_POLICY_FILE_SUFFIX: &str = "MANAGED_POLICY_FILE";

/// Runs `t3 policy show|check|settings|templates` natively unless `RUST_ANYTHING_UNIT_POLICY=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_POLICY) {
        return delegate_to_node(UNIT_POLICY, args);
    }

    let Some(subcmd) = args.get(1).map(String::as_str) else {
        return fail_with_command_help(&["policy"], "Missing subcommand for `t3 policy`.");
    };
    let rest = args.get(2..).unwrap_or(&[]);

    match subcmd {
        "show" => match parse_show_flags(rest) {
            Ok(base_dir) => run_show(base_dir.as_deref()),
            Err(()) => fail_with_command_help(
                &["policy", "show"],
                "Invalid or unrecognized arguments for `t3 policy show`.",
            ),
        },
        "check" => match parse_check_args(rest) {
            Some(file) => run_check(&file),
            None => fail_with_command_help(
                &["policy", "check"],
                "Missing or invalid <file> argument for `t3 policy check`.",
            ),
        },
        "settings" => run_settings(rest),
        "templates" => match parse_templates_args(rest) {
            Some((directory, policy)) => run_templates(&directory, policy.as_deref()),
            None => fail_with_command_help(
                &["policy", "templates"],
                "Missing or invalid arguments for `t3 policy templates`.",
            ),
        },
        other => fail_with_command_help(
            &["policy"],
            &format!("Unknown subcommand \"{other}\" for \"t3 policy\""),
        ),
    }
}

fn parse_show_flags(rest: &[String]) -> Result<Option<String>, ()> {
    let mut base_dir = None;
    let mut idx = 0;
    while idx < rest.len() {
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "base-dir") {
            base_dir = Some(val);
        } else {
            return Err(());
        }
        idx += 1;
    }
    Ok(base_dir)
}

fn parse_check_args(rest: &[String]) -> Option<String> {
    if rest.len() != 1 || rest[0].starts_with('-') {
        return None;
    }
    Some(rest[0].clone())
}

fn parse_templates_args(rest: &[String]) -> Option<(String, Option<String>)> {
    let mut directory = None;
    let mut policy = None;
    let mut idx = 0;

    while idx < rest.len() {
        let arg = &rest[idx];
        if let Ok(Some(val)) = take_flag_value(rest, &mut idx, "policy") {
            policy = Some(val);
        } else if arg.starts_with('-') || directory.is_some() {
            return None;
        } else {
            directory = Some(arg.clone());
        }
        idx += 1;
    }

    directory.map(|dir| (dir, policy))
}

fn run_check(file: &str) -> i32 {
    if !Path::new(file).exists() {
        eprintln!("No policy file at {file}.");
        return 1;
    }
    let doc = match load_policy_document(file) {
        Ok(doc) => doc,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    match describe_policy(file, Some(&doc)) {
        Ok(text) => {
            let mut out = BufWriter::with_capacity(8192, std::io::stdout().lock());
            if let Err(err) = writeln!(out, "{text}").and_then(|()| out.flush()) {
                eprintln!("Failed to write policy output: {err}");
                return 1;
            }
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn run_show(base_dir: Option<&str>) -> i32 {
    let policy_path = resolve_managed_policy_path();
    let (doc, installed) = if Path::new(&policy_path).exists() {
        let loaded = match load_policy_document(&policy_path) {
            Ok(doc) => doc,
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        };
        let fingerprint = match policy_fingerprint_12hex(&loaded) {
            Ok(fp) => fp,
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        };
        let summary = LoadedPolicySummary {
            source: policy_path.clone(),
            organization: loaded.organization.clone(),
            fingerprint,
        };
        (Some(loaded), Some(summary))
    } else {
        (None, None)
    };

    match describe_policy(&policy_path, doc.as_ref()) {
        Ok(text) => println!("{text}"),
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    }

    let (_, paths) = derive_paths(base_dir);
    let running = read_live_server_runtime_state(&paths.server_runtime_state_path).map(|state| {
        let managed_policy = match state.managed_policy {
            ManagedPolicyState::Unspecified => None,
            ManagedPolicyState::Null => Some(None),
            ManagedPolicyState::Loaded(record) => Some(Some(LoadedPolicySummary {
                source: record.source,
                organization: record.organization,
                fingerprint: record.fingerprint,
            })),
        };
        RunningServerState {
            pid: u32::try_from(state.pid).unwrap_or(0),
            started_at: state.started_at,
            managed_policy,
        }
    });

    println!(
        "{}",
        describe_running_server(running.as_ref(), installed.as_ref())
    );
    0
}

fn run_settings(rest: &[String]) -> i32 {
    if let Some(unexpected) = rest.first() {
        let msg = if unexpected.starts_with('-') {
            format!("Unrecognized flag: {unexpected} in command t3 policy settings")
        } else {
            format!("Unexpected argument: {unexpected}")
        };
        return fail_with_command_help(&["policy", "settings"], &msg);
    }
    write_stdout_line(&describe_manageable_settings(), "policy settings")
}

fn run_templates(directory: &str, policy_file: Option<&str>) -> i32 {
    let document = if let Some(file) = policy_file {
        if !Path::new(file).exists() {
            eprintln!("No policy file at {file}.");
            return 1;
        }
        match load_policy_document(file) {
            Ok(doc) => doc,
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        }
    } else {
        sample_policy()
    };

    let profile_uuid = Uuid::new_v4().to_string();
    let payload_uuid = Uuid::new_v4().to_string();
    let profile = match macos_profile(&document, &profile_uuid, &payload_uuid) {
        Ok(plist) => plist,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };

    let root = Path::new(directory);
    let admx_path = root.join("windows").join("t3code.admx");
    let adml_path = root.join("windows").join("en-US").join("t3code.adml");
    let macos_path = root.join("macos").join("t3code.mobileconfig");

    let files = [
        (admx_path, windows_admx()),
        (adml_path, windows_adml()),
        (macos_path, profile),
    ];

    for (path, contents) in files {
        if let Some(parent) = path.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            eprintln!("Failed to create directory {}: {err}", parent.display());
            return 1;
        }
        if let Err(err) = fs::write(&path, contents) {
            eprintln!("Failed to write {}: {err}", path.display());
            return 1;
        }
        println!("{}", path.display());
    }
    0
}

/// Resolves the managed policy file path from `<BRAND>_MANAGED_POLICY_FILE` or the platform default.
pub fn resolve_managed_policy_path() -> String {
    if let Some(val) = ACTIVE_BRAND.read_env(MANAGED_POLICY_FILE_SUFFIX) {
        return val.trim().to_owned();
    }
    default_managed_policy_path().to_owned()
}

const fn default_managed_policy_path() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        r"C:\Program Files\T3Code\policy.json"
    }
    #[cfg(target_os = "macos")]
    {
        "/Library/Application Support/T3Code/policy.json"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "/etc/t3code/policy.json"
    }
}

pub(crate) fn load_policy_document(path: &str) -> Result<ManagedPolicyDocument, String> {
    let raw = fs::read_to_string(path).map_err(|err| {
        format!(
            "T3 Code cannot use the managed policy at {path}: the file cannot be read ({err}). It will not start until an administrator fixes this."
        )
    })?;
    serde_json::from_str::<ManagedPolicyDocument>(&raw).map_err(|err| {
        format!(
            "T3 Code cannot use the managed policy at {path}: it is not valid JSON or does not match the policy schema ({err}). It will not start until an administrator fixes this."
        )
    })
}

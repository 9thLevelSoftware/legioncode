//! Standalone `legioncode __service-launcher` entrypoint backed by `legion_persistence::service_launcher`.

use std::collections::HashMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use legion_persistence::service_launcher::{
    LauncherStartupDecision, SERVICE_ENV_FILE, SERVICE_LAUNCHER_CONTEXT_ENV, SERVICE_UNIT_ENV,
    ServiceLockGuard, parse_service_launcher_args, plan_launcher_startup, read_service_state,
    service_launcher_lock_path, service_serving_lock_path, service_state_path,
};

use crate::cli_global::fail_with_command_help;
use crate::cmd_status::parse_service_env;
use crate::fallback::{delegate_to_node, expand_home, unit_forced_original};

/// Unit identifier for `RUST_ANYTHING_UNIT_SERVICE_LAUNCHER` and fallback logs.
pub const UNIT_SERVICE_LAUNCHER: &str = "SERVICE_LAUNCHER";

/// Runs `legioncode __service-launcher` natively unless `RUST_ANYTHING_UNIT_SERVICE_LAUNCHER=original`.
pub fn run(args: &[String]) -> i32 {
    if unit_forced_original(UNIT_SERVICE_LAUNCHER) {
        return delegate_to_node(UNIT_SERVICE_LAUNCHER, args);
    }

    let rest = args.get(1..).unwrap_or(&[]);
    let Some(options) = parse_service_launcher_args(rest) else {
        return fail_with_command_help(
            &["__service-launcher"],
            "Invalid arguments for `legioncode __service-launcher`.",
        );
    };

    let base_raw = options
        .base_dir
        .clone()
        .or_else(|| env::var("LEGIONCODE_HOME").ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let Some(base_raw) = base_raw else {
        eprintln!("LEGIONCODE_HOME is required by the LegionCode service launcher.");
        return 1;
    };
    let base_dir = expand_home(&base_raw);

    let log_file = options
        .log_file
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    launch_locked(&base_dir, options.service_unit.as_deref(), log_file)
}

fn launch_locked(base_dir: &Path, service_unit: Option<&str>, log_file: Option<&str>) -> i32 {
    let state_path = service_state_path(base_dir);
    let state = match read_service_state(&state_path) {
        Ok(st) => st,
        Err(err) => {
            log_launcher_msg(log_file, &format!("[service-launcher] {err}"));
            return 1;
        }
    };

    let launcher_lock =
        match ServiceLockGuard::try_acquire(&service_launcher_lock_path(base_dir), 5_000) {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                log_launcher_msg(
                    log_file,
                    "[service-launcher] Another service launcher already holds the launcher lock.",
                );
                return 1;
            }
            Err(err) => {
                log_launcher_msg(log_file, &format!("[service-launcher] {err}"));
                return 1;
            }
        };

    let serving_lock =
        match ServiceLockGuard::try_acquire(&service_serving_lock_path(base_dir), 5_000) {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                log_launcher_msg(
                    log_file,
                    "[service-launcher] Service serving lock is already held.",
                );
                return 1;
            }
            Err(err) => {
                log_launcher_msg(log_file, &format!("[service-launcher] {err}"));
                return 1;
            }
        };

    let decision = match plan_launcher_startup(base_dir, &state) {
        Ok(d) => d,
        Err(err) => {
            log_launcher_msg(log_file, &format!("[service-launcher] {err}"));
            return 1;
        }
    };

    let plan = match decision {
        LauncherStartupDecision::Launch(plan) => plan,
        LauncherStartupDecision::RollbackRequired {
            update_id,
            previous_version,
            reason,
        } => {
            log_launcher_msg(
                log_file,
                &format!(
                    "[service-launcher] Rollback required for update {update_id} -> {previous_version}: {reason}"
                ),
            );
            return 1;
        }
        LauncherStartupDecision::Blocked { version, reason } => {
            log_launcher_msg(
                log_file,
                &format!("[service-launcher] Startup blocked for legioncode@{version}: {reason}"),
            );
            return 1;
        }
    };

    let service_env = load_service_env(base_dir);
    let code = spawn_managed_child(base_dir, &plan, &service_env, service_unit, log_file);
    drop(serving_lock);
    drop(launcher_lock);
    code
}

fn spawn_managed_child(
    base_dir: &Path,
    plan: &legion_persistence::service_launcher::LauncherChildPlan,
    service_env: &HashMap<String, String>,
    service_unit: Option<&str>,
    log_file: Option<&str>,
) -> i32 {
    let mut executables = vec![plan.command.clone()];
    if let Ok(self_exe) = env::current_exe()
        && self_exe != plan.command
    {
        executables.push(self_exe);
    }

    for exe in executables {
        let mut cmd = Command::new(&exe);
        cmd.args(&plan.args);
        cmd.arg("--base-dir").arg(base_dir);
        cmd.envs(service_env);
        cmd.env("LEGIONCODE_HOME", base_dir);
        cmd.env(SERVICE_LAUNCHER_CONTEXT_ENV, &plan.context_json);
        if let Some(unit) = service_unit {
            cmd.env(SERVICE_UNIT_ENV, unit);
        }
        configure_child_stdio(&mut cmd, log_file);
        if let Ok(mut child) = cmd.spawn() {
            return match child.wait() {
                Ok(status) => status.code().unwrap_or(1),
                Err(err) => {
                    log_launcher_msg(log_file, &format!("[service-launcher] Wait failed: {err}"));
                    1
                }
            };
        }
    }

    log_launcher_msg(
        log_file,
        &format!(
            "[service-launcher] Failed to launch service runtime at '{}'.",
            plan.command.display()
        ),
    );
    1
}

fn configure_child_stdio(cmd: &mut Command, log_file: Option<&str>) {
    cmd.stdin(Stdio::null());
    if let Some(path) = log_file
        && let Ok(out) = OpenOptions::new().create(true).append(true).open(path)
        && let Ok(err) = out.try_clone()
    {
        cmd.stdout(Stdio::from(out));
        cmd.stderr(Stdio::from(err));
        return;
    }
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
}

fn load_service_env(base_dir: &Path) -> HashMap<String, String> {
    let path = base_dir.join("runtime").join(SERVICE_ENV_FILE);
    match fs::read_to_string(path) {
        Ok(raw) => parse_service_env(&raw),
        Err(_) => HashMap::new(),
    }
}

fn log_launcher_msg(log_file: Option<&str>, msg: &str) {
    if let Some(path) = log_file
        && let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{msg}");
    }
    eprintln!("{msg}");
}

//! Linux systemd memory and OOM protection plan & drop-in renderers (`serviceProtection.ts`).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use t3_contracts::brand::{ACTIVE_BRAND, BrandConfig};

use crate::boot_service_units::protect_drop_in_for_brand;

const fn str_eq(left: &str, right: &str) -> bool {
    let a = left.as_bytes();
    let b = right.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut idx = 0;
    while idx < a.len() {
        if a[idx] != b[idx] {
            return false;
        }
        idx += 1;
    }
    true
}

/// Drop-in configuration filename applied to cgroup slices and services (`serviceProtection.ts:12`).
pub const PROTECT_DROP_IN: &str = if str_eq(ACTIVE_BRAND.url_scheme, "legioncode") {
    "90-legioncode-protect.conf"
} else {
    "90-t3code-protect.conf"
};

/// Default host system systemd unit directory (`serviceProtection.ts:13`).
pub const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";

/// Remote-access daemons protected alongside the T3 Code service (`serviceProtection.ts:14`).
pub const REMOTE_ACCESS_UNITS: &[&str] = &["tailscaled.service", "ssh.service", "sshd.service"];

const USER_CHAIN_LOW: &str = "MemoryLow=2G";
const PROTECTED_OOM_SCORE: &str = "OOMScoreAdjust=-900";

/// Mode passed to [`render_protection_privileged_script`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionMode {
    /// Install/reload system drop-ins and adjust running `oom_score_adj`.
    Apply,
    /// Remove drop-ins owned by this user (and shared drop-ins if no other users remain).
    Undo,
}

/// Single systemd `.d/90-t3code-protect.conf` drop-in unit (`serviceProtection.ts:21-27`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemdDropIn {
    /// Target systemd unit name (e.g. `user@1000.service` or `t3code.service`).
    pub unit: String,
    /// Absolute path of the drop-in file.
    pub path: String,
    /// Complete INI contents including trailing newline.
    pub contents: String,
    /// Host-wide drop-in retained while another user's protection remains active.
    #[serde(default)]
    pub shared: bool,
}

/// Complete Linux cgroup memory and OOM protection plan (`serviceProtection.ts:29-41`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceProtectionPlan {
    /// Directory hosting system-wide systemd unit drop-ins.
    pub system_unit_dir: String,
    /// System-wide drop-ins written with elevated privileges (`sudo`).
    pub system: Vec<SystemdDropIn>,
    /// User manager default drop-in (`user.conf.d/90-t3code-protect.conf`).
    pub manager_defaults: SystemdDropIn,
    /// Per-user slice, scope, and `t3code.service` drop-ins.
    pub user: Vec<SystemdDropIn>,
    /// System units whose running main process receives its configured OOM score immediately.
    pub oom_units: Vec<String>,
}

fn build_drop_in(
    directory: &str,
    unit: &str,
    section: &str,
    properties: &[&str],
    shared: bool,
    drop_in_file: &str,
) -> SystemdDropIn {
    let mut contents = format!("[{section}]\n");
    for prop in properties {
        contents.push_str(prop);
        contents.push('\n');
    }
    SystemdDropIn {
        unit: unit.to_owned(),
        path: format!("{directory}/{unit}.d/{drop_in_file}"),
        contents,
        shared,
    }
}

/// Builds a [`ServiceProtectionPlan`] using [`ACTIVE_BRAND`] (`protectionPlan`).
pub fn build_service_protection_plan(
    uid: u32,
    user_unit_dir: &str,
    remote_access_units: &[&str],
    system_unit_dir: Option<&str>,
) -> ServiceProtectionPlan {
    build_service_protection_plan_for_brand(
        uid,
        user_unit_dir,
        remote_access_units,
        system_unit_dir,
        &ACTIVE_BRAND,
    )
}

/// Builds a [`ServiceProtectionPlan`] parameterized by `brand` (`protectionPlan` in `serviceProtection.ts:60-96`).
pub fn build_service_protection_plan_for_brand(
    uid: u32,
    user_unit_dir: &str,
    remote_access_units: &[&str],
    system_unit_dir: Option<&str>,
    brand: &BrandConfig,
) -> ServiceProtectionPlan {
    let drop_in = protect_drop_in_for_brand(brand);
    let manager = format!("user@{uid}.service");
    let sys_dir = system_unit_dir.unwrap_or(SYSTEM_UNIT_DIR);
    let user_slice = format!("user-{uid}.slice");

    let mut system = vec![
        build_drop_in(
            sys_dir,
            "user.slice",
            "Slice",
            &[USER_CHAIN_LOW],
            true,
            &drop_in,
        ),
        build_drop_in(
            sys_dir,
            &user_slice,
            "Slice",
            &[USER_CHAIN_LOW],
            false,
            &drop_in,
        ),
        build_drop_in(
            sys_dir,
            &manager,
            "Service",
            &[USER_CHAIN_LOW, PROTECTED_OOM_SCORE],
            false,
            &drop_in,
        ),
    ];
    if !remote_access_units.is_empty() {
        system.push(build_drop_in(
            sys_dir,
            "system.slice",
            "Slice",
            &["MemoryLow=512M"],
            true,
            &drop_in,
        ));
        for unit in remote_access_units {
            system.push(build_drop_in(
                sys_dir,
                unit,
                "Service",
                &["MemoryLow=256M", PROTECTED_OOM_SCORE],
                true,
                &drop_in,
            ));
        }
    }

    let config_root = user_unit_dir.strip_suffix("/user").unwrap_or(user_unit_dir);
    let manager_defaults = SystemdDropIn {
        unit: "user.conf".to_owned(),
        path: format!("{config_root}/user.conf.d/{drop_in}"),
        contents: "[Manager]\nDefaultOOMScoreAdjust=200\n".to_owned(),
        shared: false,
    };

    let user = vec![
        build_drop_in(
            user_unit_dir,
            "app.slice",
            "Slice",
            &["MemoryLow=1536M"],
            false,
            &drop_in,
        ),
        build_drop_in(
            user_unit_dir,
            "init.scope",
            "Scope",
            &["MemoryLow=256M"],
            false,
            &drop_in,
        ),
        build_drop_in(
            user_unit_dir,
            brand.systemd_unit_name,
            "Service",
            &[PROTECTED_OOM_SCORE],
            false,
            &drop_in,
        ),
    ];

    let mut oom_units = Vec::with_capacity(1 + remote_access_units.len());
    oom_units.push(manager);
    for unit in remote_access_units {
        oom_units.push((*unit).to_owned());
    }

    ServiceProtectionPlan {
        system_unit_dir: sys_dir.to_owned(),
        system,
        manager_defaults,
        user,
        oom_units,
    }
}

fn quote_shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn directory_of(file: &str) -> &str {
    file.rfind('/').map_or("", |idx| &file[..idx])
}

/// Renders the POSIX shell script executed via `sudo` to apply or undo system drop-ins
/// using [`ACTIVE_BRAND`] (`serviceProtection.ts:101-142`).
pub fn render_protection_privileged_script(
    plan: &ServiceProtectionPlan,
    mode: ProtectionMode,
) -> String {
    render_protection_privileged_script_for_brand(plan, mode, &ACTIVE_BRAND)
}

/// Renders the POSIX shell script executed via `sudo` to apply or undo system drop-ins
/// parameterized by `brand` (`serviceProtection.ts:101-142`).
pub fn render_protection_privileged_script_for_brand(
    plan: &ServiceProtectionPlan,
    mode: ProtectionMode,
    brand: &BrandConfig,
) -> String {
    let drop_in = protect_drop_in_for_brand(brand);
    let heredoc_tag = format!("{}PROTECT", brand.primary_env_prefix);
    let mut lines = vec!["set -e".to_owned()];
    let push_remove = |lines: &mut Vec<String>, drop_ins: &[&SystemdDropIn], indent: &str| {
        for drop in drop_ins {
            lines.push(format!("{indent}rm -f {}", quote_shell(&drop.path)));
            lines.push(format!(
                "{indent}rmdir {} 2>/dev/null || true",
                quote_shell(directory_of(&drop.path))
            ));
        }
    };

    match mode {
        ProtectionMode::Apply => {
            for drop in &plan.system {
                lines.push(format!(
                    "mkdir -p {}",
                    quote_shell(directory_of(&drop.path))
                ));
                lines.push(format!(
                    "cat >{} <<'{heredoc_tag}'",
                    quote_shell(&drop.path)
                ));
                lines.push(drop.contents.trim_end().to_owned());
                lines.push(heredoc_tag.clone());
            }
        }
        ProtectionMode::Undo => {
            let owned: Vec<&SystemdDropIn> = plan.system.iter().filter(|d| !d.shared).collect();
            push_remove(&mut lines, &owned, "");
            let other_users = format!(
                "{}/user@*.service.d/{drop_in}",
                quote_shell(&plan.system_unit_dir)
            );
            lines.push(format!("set -- {other_users}"));
            lines.push("if [ ! -e \"$1\" ]; then".to_owned());
            let shared: Vec<&SystemdDropIn> = plan.system.iter().filter(|d| d.shared).collect();
            push_remove(&mut lines, &shared, "  ");
            lines.push("fi".to_owned());
        }
    }

    let oom_args: Vec<String> = plan.oom_units.iter().map(|u| quote_shell(u)).collect();
    lines.push("systemctl daemon-reload".to_owned());
    lines.push(format!("for unit in {}; do", oom_args.join(" ")));
    lines.push("  pid=$(systemctl show --property=MainPID --value \"$unit\")".to_owned());
    lines.push("  if [ \"${pid:-0}\" -gt 0 ]; then".to_owned());
    lines.push(
        "    systemctl show --property=OOMScoreAdjust --value \"$unit\" >\"/proc/$pid/oom_score_adj\" || true"
            .to_owned(),
    );
    lines.push("  fi".to_owned());
    lines.push("done".to_owned());
    lines.push(String::new());
    lines.join("\n")
}

/// Extracts unique loaded remote-access unit names from `systemctl show --property=Id,LoadState`
/// output (`serviceProtection.ts:175-181`).
pub fn parse_loaded_remote_access_units(systemctl_show: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut current: IndexMap<&str, &str> = IndexMap::new();

    let flush_block = |map: &mut IndexMap<&str, &str>, out: &mut Vec<String>| {
        if map.get("LoadState").copied() == Some("loaded")
            && let Some(&id) = map.get("Id")
            && REMOTE_ACCESS_UNITS.contains(&id)
            && !out.iter().any(|existing| existing == id)
        {
            out.push(id.to_owned());
        }
        map.clear();
    };

    for raw_line in systemctl_show.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            flush_block(&mut current, &mut seen);
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            current.insert(key, value);
        }
    }
    flush_block(&mut current, &mut seen);
    seen
}

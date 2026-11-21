//! Linux-specific AppArmor userns, shared-library, and Fontconfig setup for `t3 browser setup`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fallback::{resolve_base_dir, write_stdout_line};
use crate::invocation::{non_empty_env, resolve_host_cli_command};

const APPARMOR_PROFILE_PATH: &str = "/etc/apparmor.d/t3-chrome-headless-shell";
const USERNS_RESTRICTION_PATH: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
const APPARMOR_PROFILE: &str = "\
# Written by `t3 browser setup`: lets T3 Code's headless browser use Chrome's sandbox.
abi <abi/4.0>,
include <tunables/global>

profile t3-chrome-headless-shell /**/tools/chrome-headless-shell/*/*/chrome-headless-shell flags=(unconfined) {
  userns,

  include if exists <local/t3-chrome-headless-shell>
}
";

const DEBIAN_PACKAGES: &[&[&str]] = &[
    &["libnss3"],
    &["libglib2.0-0t64", "libglib2.0-0"],
    &["libatk1.0-0t64", "libatk1.0-0"],
    &["libatk-bridge2.0-0t64", "libatk-bridge2.0-0"],
    &["libatspi2.0-0t64", "libatspi2.0-0"],
    &["libdbus-1-3"],
    &["libx11-6"],
    &["libxcb1"],
    &["libxcomposite1"],
    &["libxdamage1"],
    &["libxext6"],
    &["libxfixes3"],
    &["libxrandr2"],
    &["libxkbcommon0"],
    &["libgbm1"],
    &["libasound2t64", "libasound2"],
    &["libexpat1"],
];

const FONT_PACKAGES: &[&str] = &["fontconfig", "fonts-liberation"];

pub(crate) fn run_linux_browser_setup(explicit_base_dir: Option<&str>) -> i32 {
    let base_dir_path = resolve_setup_base_dir(explicit_base_dir);
    let is_root = current_linux_uid() == Some(0);
    let sudo_user = non_empty_env("SUDO_USER");
    let setup_args = if explicit_base_dir.is_some() || non_empty_env("T3CODE_HOME").is_some() {
        vec![
            "--base-dir".to_owned(),
            base_dir_path.to_string_lossy().into_owned(),
        ]
    } else {
        Vec::new()
    };
    let setup_cmd = format!(
        "sudo {}",
        resolve_host_cli_command("browser setup", &setup_args)
    );
    let sandbox_disabled = non_empty_env("T3CODE_SERVER_BROWSER_SANDBOX").as_deref() == Some("0");
    let root_warning = (is_root && sudo_user.as_deref().unwrap_or("root") == "root" && !sandbox_disabled)
        .then_some(
            "T3 server is running as root; Chrome's sandbox cannot start. Run T3 as a nonroot user. Otherwise set `T3CODE_SERVER_BROWSER_SANDBOX=0` in the T3 server's environment and restart the server; this disables Chrome's sandbox. Manual: `T3CODE_SERVER_BROWSER_SANDBOX=0 t3 serve`. Systemd: add `Environment=T3CODE_SERVER_BROWSER_SANDBOX=0` under `[Service]`, reload systemd, then restart the service. `t3 browser setup` cannot fix root sandboxing.",
        );

    let restricted = linux_userns_restricted();
    let needs_profile = if is_root {
        restricted
    } else {
        restricted && !Path::new(APPARMOR_PROFILE_PATH).exists()
    };
    let reload_hint = if restricted && !is_root {
        format!(
            "\nIf the browser still reports a blocked sandbox, the AppArmor profile at {APPARMOR_PROFILE_PATH} is not loaded. To load it, run:\n\n  {setup_cmd}\n"
        )
    } else {
        String::new()
    };
    let browser = installed_browser(&base_dir_path);
    let missing = browser
        .as_deref()
        .map(missing_libraries)
        .unwrap_or_default();
    let needs_fonts = !font_available();

    if !needs_profile && missing.is_empty() && !needs_fonts {
        let base_msg = root_warning.unwrap_or_else(|| {
            if browser.is_some() {
                "This host is ready for T3's browser."
            } else {
                "Chrome's sandbox is allowed here. T3's browser installs on first use; if it then reports missing libraries, run this again."
            }
        });
        return write_stdout_line(&format!("{base_msg}{reload_hint}"), "browser setup");
    }

    if !is_root {
        if needs_profile {
            println!(
                "This host blocks the sandbox T3's browser runs in. Setup installs an AppArmor profile at {APPARMOR_PROFILE_PATH} that allows it."
            );
        }
        if !missing.is_empty() {
            println!(
                "T3's browser is missing {}; setup installs them.",
                missing.join(", ")
            );
        }
        if needs_fonts {
            println!("T3's browser needs fontconfig and system fonts; setup installs them.");
        }
        return write_stdout_line(
            &format!("\nThis needs root. Run:\n\n  {setup_cmd}\n"),
            "browser setup",
        );
    }

    run_linux_root_install(needs_profile, &missing, needs_fonts)
}

fn resolve_setup_base_dir(explicit: Option<&str>) -> PathBuf {
    if explicit.is_some() || non_empty_env("T3CODE_HOME").is_some() {
        return resolve_base_dir(explicit).0;
    }
    if let Some(sudo_user) = non_empty_env("SUDO_USER")
        && let Ok(out) = Command::new("getent").args(["passwd", &sudo_user]).output()
        && out.status.success()
        && let Some(home) = String::from_utf8_lossy(&out.stdout)
            .trim()
            .split(':')
            .nth(5)
        && !home.is_empty()
    {
        return Path::new(home).join(".t3");
    }
    resolve_base_dir(None).0
}

fn current_linux_uid() -> Option<u32> {
    let out = Command::new("id").arg("-u").output().ok()?;
    out.status
        .success()
        .then(|| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u32>()
                .ok()
        })
        .flatten()
}

fn linux_userns_restricted() -> bool {
    std::fs::read_to_string(USERNS_RESTRICTION_PATH).is_ok_and(|s| s.trim() == "1")
}

fn installed_browser(base_dir: &Path) -> Option<PathBuf> {
    let root = base_dir.join("tools").join("chrome-headless-shell");
    for plat in std::fs::read_dir(&root).ok()?.flatten() {
        let Ok(versions) = std::fs::read_dir(plat.path()) else {
            continue;
        };
        for ver in versions.flatten() {
            let exe = ver.path().join("chrome-headless-shell");
            if exe.exists() {
                return Some(exe);
            }
        }
    }
    None
}

fn missing_libraries(executable: &Path) -> Vec<String> {
    let Ok(out) = Command::new(executable).arg("--version").output() else {
        return Vec::new();
    };
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let Some(marker) = combined.find("error while loading shared libraries: ") else {
        return Vec::new();
    };
    let rest = &combined[marker + "error while loading shared libraries: ".len()..];
    let loader_err = rest.split([':', ' ', '\r', '\n']).next().unwrap_or("");
    let mut listed = Vec::new();
    if let Ok(ldd_out) = Command::new("ldd").arg(executable).output() {
        for line in String::from_utf8_lossy(&ldd_out.stdout).lines() {
            if let Some((lib, target)) = line.trim().split_once(" => ")
                && target.trim() == "not found"
            {
                listed.push(lib.trim().to_owned());
            }
        }
    }
    if listed.is_empty() && !loader_err.is_empty() {
        listed.push(loader_err.to_owned());
    }
    listed
}

fn font_available() -> bool {
    Command::new("fc-match")
        .args(["--format=%{file}", "sans-serif"])
        .output()
        .is_ok_and(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
}

fn run_linux_root_install(needs_profile: bool, missing: &[String], needs_fonts: bool) -> i32 {
    if needs_profile && install_apparmor_profile() != 0 {
        return 1;
    }
    if !missing.is_empty() || needs_fonts {
        return install_apt_deps(missing, needs_fonts);
    }
    0
}

fn install_apparmor_profile() -> i32 {
    let on_disk = Path::new(APPARMOR_PROFILE_PATH).exists();
    if !on_disk && let Err(err) = std::fs::write(APPARMOR_PROFILE_PATH, APPARMOR_PROFILE) {
        eprintln!("Could not write {APPARMOR_PROFILE_PATH}: {err}");
        return 1;
    }
    if run_cmd(
        "load the AppArmor profile",
        "apparmor_parser",
        &["-r", APPARMOR_PROFILE_PATH],
    ) != 0
    {
        return 1;
    }
    let verb = if on_disk {
        "Reloaded"
    } else {
        "Wrote and loaded"
    };
    println!(
        "{verb} the AppArmor profile at {APPARMOR_PROFILE_PATH} that allows Chrome's sandbox for T3's browser."
    );
    0
}

fn install_apt_deps(missing: &[String], needs_fonts: bool) -> i32 {
    if !Path::new("/usr/bin/apt-get").exists() {
        if !missing.is_empty() {
            println!(
                "T3's browser is missing {}. Install them with your package manager, then run this again.",
                missing.join(", ")
            );
        }
        if needs_fonts {
            println!(
                "T3's browser needs fontconfig and system fonts. Install them with your package manager, then run this again."
            );
        }
        return 0;
    }
    if run_cmd("refresh the package lists", "apt-get", &["update"]) != 0 {
        return 1;
    }
    if !missing.is_empty() {
        let mut args = vec!["install", "-y", "--no-install-recommends"];
        args.extend(DEBIAN_PACKAGES.iter().map(|opts| pick_apt_pkg(opts)));
        if run_cmd("install the browser's libraries", "apt-get", &args) != 0 {
            return 1;
        }
        println!("Installed the browser's libraries.");
    }
    if needs_fonts {
        let mut args = vec!["install", "-y", "--no-install-recommends"];
        args.extend_from_slice(FONT_PACKAGES);
        if run_cmd("install the browser's fonts", "apt-get", &args) != 0 {
            return 1;
        }
        if !font_available() {
            println!(
                "Font packages are installed, but Fontconfig still finds no system font. Check its configuration and run setup again."
            );
            return 0;
        }
        println!(
            "Installed the browser's fonts. Restart the T3 server if browser tabs were already open."
        );
    }
    0
}

fn pick_apt_pkg(candidates: &[&'static str]) -> &'static str {
    for &name in candidates {
        if let Ok(out) = Command::new("apt-cache").args(["policy", name]).output()
            && out.status.success()
            && String::from_utf8_lossy(&out.stdout).lines().any(|line| {
                line.trim()
                    .strip_prefix("Candidate:")
                    .map(str::trim)
                    .is_some_and(|v| !v.is_empty() && v != "(none)")
            })
        {
            return name;
        }
    }
    candidates[0]
}

fn run_cmd(step: &str, cmd: &str, args: &[&str]) -> i32 {
    match Command::new(cmd).args(args).status() {
        Ok(s) if s.success() => 0,
        Ok(s) => {
            eprintln!(
                "Could not {step}: {cmd} exited with {}",
                s.code().unwrap_or(1)
            );
            1
        }
        Err(err) => {
            eprintln!("Could not {step}: {err}");
            1
        }
    }
}

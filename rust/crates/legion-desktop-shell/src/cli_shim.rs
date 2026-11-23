//! `AppImage` CLI wrapper shim renderer keeping systemd stops and closed stdin clean (`#40624`).

use legion_contracts::brand::{ACTIVE_BRAND, BrandConfig};

/// Renders the POSIX `sh` `AppImage` CLI wrapper matching `apps/desktop/src/app/DesktopCliShim.ts`,
/// ignoring `TERM` in the wrapper until the mounted CLI exits, lazily unmounting on `EXIT`,
/// forwarding `TERM`/`HUP` to the CLI child, and preserving stdin across backgrounding via fd `9`
/// (`#40624`).
#[must_use]
pub fn render_appimage_cli_shim(app_image_path: &str, home_dir_name: Option<&str>) -> String {
    render_appimage_cli_shim_for_brand(app_image_path, home_dir_name, &ACTIVE_BRAND)
}

/// Brand-parameterized variant of [`render_appimage_cli_shim`].
#[must_use]
pub fn render_appimage_cli_shim_for_brand(
    app_image_path: &str,
    home_dir_name: Option<&str>,
    brand: &BrandConfig,
) -> String {
    let quoted_app = shell_quote(app_image_path);
    let quoted_home = home_dir_name.map(shell_quote);
    let primary_home_var = format!("{}HOME", brand.primary_env_prefix);
    let run_invocation = r#""$mount/AppRun" "$@""#;
    let mut lines = vec!["#!/bin/sh".to_owned(), format!("app={quoted_app}")];
    if let Some(home) = quoted_home {
        lines.push(format!(r#"if [ -z "${{{primary_home_var}:-}}" ]; then"#));
        lines.push(format!(r"  export {primary_home_var}={home}"));
        lines.push("fi".to_owned());
    }
    lines.extend([
        // Extract-and-run would make the runtime start the app instead of mounting it.
        "unset APPIMAGE_EXTRACT_AND_RUN".to_owned(),
        // Its own session keeps a group signal (Ctrl-C, SSH hangup) from unmounting it under the
        // running server, and ignoring TERM survives a stop sent to every process (systemd's default).
        // Without setsid the extract fallback runs.
        "work=$(mktemp -d) || exit 1".to_owned(),
        "trap '' TERM".to_owned(),
        r#"setsid "$app" --appimage-mount >"$work/mount" 2>/dev/null &"#.to_owned(),
        "mounter=$!".to_owned(),
        // It unmounts itself on TERM, which it now ignores, so the trap unmounts it. Lazily, like the
        // runtime: the server's own children may still be exiting.
        r#"trap '[ -z "$mount" ] || fusermount3 -uz "$mount" 2>/dev/null || fusermount -uz "$mount" 2>/dev/null; kill -HUP "$mounter" 2>/dev/null; rm -rf "$work"' EXIT"#.to_owned(),
        "trap 'exit 130' INT TERM HUP".to_owned(),
        "mount=".to_owned(),
        r#"while kill -0 "$mounter" 2>/dev/null; do"#.to_owned(),
        r#"  if [ -s "$work/mount" ]; then"#.to_owned(),
        r#"    IFS= read -r mount <"$work/mount""#.to_owned(),
        r#"    [ -z "$mount" ] || break"#.to_owned(),
        "  fi".to_owned(),
        "  sleep 0.05".to_owned(),
        "done".to_owned(),
        r#"if [ -z "$mount" ] || ! kill -0 "$mounter" 2>/dev/null; then"#.to_owned(),
        "  exit 1".to_owned(),
        "fi".to_owned(),
        // The runtime sets this when it launches the app; the CLI skips its path-keyed compile cache on it.
        r#"export APPIMAGE="$app""#.to_owned(),
        // The shell must outlive the CLI to unmount, so it forwards TERM and HUP; the terminal sends
        // Ctrl-C to the CLI directly. A background child would read /dev/null, so pass stdin on fd 9.
        // With stdin closed, `exec 9<&0` would end the shell; `command` makes it a normal error.
        "{ command exec 9<&0; } 2>/dev/null || exec 9</dev/null".to_owned(),
        format!("{run_invocation} <&9 9<&- &"),
        "cli=$!".to_owned(),
        "exec 9<&-".to_owned(),
        r#"trap 'kill -TERM "$cli" 2>/dev/null' TERM"#.to_owned(),
        r#"trap 'kill -HUP "$cli" 2>/dev/null' HUP"#.to_owned(),
        "trap '' INT".to_owned(),
        "status=0".to_owned(),
        r#"wait "$cli" || status=$?"#.to_owned(),
        // A trapped signal interrupts wait; wait again for the CLI's own exit status.
        r#"while kill -0 "$cli" 2>/dev/null; do"#.to_owned(),
        "  status=0".to_owned(),
        r#"  wait "$cli" || status=$?"#.to_owned(),
        "done".to_owned(),
        r#"exit "$status""#.to_owned(),
        String::new(),
    ]);
    lines.join("\n")
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

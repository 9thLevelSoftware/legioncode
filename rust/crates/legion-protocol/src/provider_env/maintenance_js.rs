//! Path-matching predicates and JS package-manager resolution for 15-tier maintenance.

use super::maintenance_types::{
    MaintenanceCapabilities, MaintenanceInstaller, MaintenanceResolutionInput, MaintenanceTier,
    ProviderMaintenanceDefinition, UpdateCommandSpec,
};

pub(super) fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

fn path_matches_any_dir(path: &str, dirs: &[String]) -> bool {
    let norm_path = normalize_slashes(path);
    let lower_path = norm_path.to_ascii_lowercase();
    dirs.iter().any(|dir| {
        let norm_dir = normalize_slashes(dir).trim_end_matches('/').to_owned();
        if norm_dir.is_empty() {
            return false;
        }
        let lower_dir = norm_dir.to_ascii_lowercase();
        lower_path == lower_dir || lower_path.starts_with(&format!("{lower_dir}/"))
    })
}

fn contains_segment(path: &str, segment: &str) -> bool {
    let norm = normalize_slashes(path).to_ascii_lowercase();
    let needle = segment.to_ascii_lowercase();
    norm.split('/').any(|part| part == needle)
}

pub(super) fn matches_js_store(
    binary: Option<&str>,
    target: Option<&str>,
    default_segments: &[&str],
    extra_dirs: &[String],
) -> bool {
    [binary, target].into_iter().flatten().any(|path| {
        path_matches_any_dir(path, extra_dirs)
            || default_segments
                .iter()
                .any(|seg| contains_segment(path, seg))
    })
}

pub(super) fn looks_like_node_modules_path(binary: Option<&str>, target: Option<&str>) -> bool {
    [binary, target]
        .into_iter()
        .flatten()
        .any(|path| contains_segment(path, "node_modules"))
}

pub(super) fn is_nix_store_path(binary: Option<&str>, target: Option<&str>) -> bool {
    [binary, target].into_iter().flatten().any(|path| {
        let norm = normalize_slashes(path);
        norm.starts_with("/nix/store/") || norm.contains("/nix/store/")
    })
}

pub(super) fn looks_like_any_js_manager_path(
    binary: Option<&str>,
    target: Option<&str>,
    input: &MaintenanceResolutionInput,
) -> bool {
    matches_js_store(
        binary,
        target,
        &[".vite-plus"],
        &input.vite_plus_global_dirs,
    ) || matches_js_store(binary, target, &[".bun"], &input.bun_global_dirs)
        || matches_js_store(binary, target, &[".pnpm", "pnpm"], &input.pnpm_global_dirs)
        || matches_js_store(binary, target, &[".yarn", "yarn"], &input.yarn_global_dirs)
        || matches_js_store(binary, target, &[".volta"], &input.volta_dirs)
        || looks_like_node_modules_path(binary, target)
}

fn derive_npm_prefix(
    binary: Option<&str>,
    target: Option<&str>,
    configured_prefix: Option<&str>,
) -> Option<String> {
    if let Some(prefix) = configured_prefix.map(str::trim).filter(|p| !p.is_empty()) {
        return Some(prefix.to_owned());
    }
    for path in [target, binary].into_iter().flatten() {
        let norm = normalize_slashes(path);
        if let Some((before, _)) = norm.split_once("/lib/node_modules/")
            && !before.is_empty()
        {
            return Some(before.to_owned());
        }
        if let Some((before, _)) = norm.split_once("/node_modules/")
            && !before.is_empty()
        {
            return Some(before.to_owned());
        }
    }
    None
}

/// Returns whether `installer` supports pinning an exact `@<version>` specifier.
///
/// Specifically returns `false` for [`MaintenanceInstaller::BunGlobal`] (BUG-14)
/// because Bun enforces `minimumReleaseAge` against pinned installs.
pub fn supports_exact_pin(installer: &MaintenanceInstaller) -> bool {
    matches!(
        installer,
        MaintenanceInstaller::VitePlusGlobal
            | MaintenanceInstaller::PnpmGlobal
            | MaintenanceInstaller::YarnGlobal
            | MaintenanceInstaller::Volta
            | MaintenanceInstaller::NpmGlobal { .. }
    )
}

pub(super) fn build_result(
    installer: MaintenanceInstaller,
    resolved_tier: MaintenanceTier,
    shim_unwrapped: bool,
    install_path: Option<String>,
    lock_key: String,
    update_command: Option<UpdateCommandSpec>,
    manual_command: Option<String>,
) -> MaintenanceCapabilities {
    MaintenanceCapabilities {
        supports_exact_pin: supports_exact_pin(&installer),
        installer,
        resolved_tier,
        shim_unwrapped,
        install_path,
        lock_key,
        update_command,
        manual_command,
    }
}

fn resolve_vite_bun_pnpm(
    def: &ProviderMaintenanceDefinition,
    npm_pkg: &str,
    binary: Option<&str>,
    target: Option<&str>,
    input: &MaintenanceResolutionInput,
    shim_unwrapped: bool,
) -> Option<MaintenanceCapabilities> {
    let install_path = binary.or(target).map(str::to_owned);
    if matches_js_store(
        binary,
        target,
        &[".vite-plus"],
        &input.vite_plus_global_dirs,
    ) {
        return Some(build_result(
            MaintenanceInstaller::VitePlusGlobal,
            MaintenanceTier::VitePlusGlobal,
            shim_unwrapped,
            install_path,
            "vite-plus-global".to_owned(),
            Some(UpdateCommandSpec {
                executable: "vp".to_owned(),
                args: vec!["i".to_owned(), "-g".to_owned(), npm_pkg.to_owned()],
            }),
            def.manual_hint.clone(),
        ));
    }
    if matches_js_store(binary, target, &[".bun"], &input.bun_global_dirs) {
        return Some(build_result(
            MaintenanceInstaller::BunGlobal,
            MaintenanceTier::BunGlobal,
            shim_unwrapped,
            install_path,
            "bun-global".to_owned(),
            Some(UpdateCommandSpec {
                executable: "bun".to_owned(),
                args: vec!["i".to_owned(), "-g".to_owned(), format!("{npm_pkg}@latest")],
            }),
            def.manual_hint.clone(),
        ));
    }
    if matches_js_store(binary, target, &[".pnpm", "pnpm"], &input.pnpm_global_dirs) {
        let mut args = vec!["add".to_owned(), "-g".to_owned()];
        for allow in &def.pnpm_allow_build {
            args.push(format!("--allow-build={allow}"));
        }
        args.push(format!("{npm_pkg}@latest"));
        return Some(build_result(
            MaintenanceInstaller::PnpmGlobal,
            MaintenanceTier::PnpmGlobal,
            shim_unwrapped,
            install_path,
            "pnpm-global".to_owned(),
            Some(UpdateCommandSpec {
                executable: "pnpm".to_owned(),
                args,
            }),
            def.manual_hint.clone(),
        ));
    }
    None
}

fn resolve_yarn_volta_npm(
    def: &ProviderMaintenanceDefinition,
    npm_pkg: &str,
    binary: Option<&str>,
    target: Option<&str>,
    input: &MaintenanceResolutionInput,
    shim_unwrapped: bool,
) -> Option<MaintenanceCapabilities> {
    let install_path = binary.or(target).map(str::to_owned);
    if matches_js_store(binary, target, &[".yarn", "yarn"], &input.yarn_global_dirs) {
        return Some(build_result(
            MaintenanceInstaller::YarnGlobal,
            MaintenanceTier::YarnGlobal,
            shim_unwrapped,
            install_path,
            "yarn-global".to_owned(),
            Some(UpdateCommandSpec {
                executable: "yarn".to_owned(),
                args: vec![
                    "global".to_owned(),
                    "add".to_owned(),
                    format!("{npm_pkg}@latest"),
                ],
            }),
            def.manual_hint.clone(),
        ));
    }
    if matches_js_store(binary, target, &[".volta"], &input.volta_dirs) {
        return Some(build_result(
            MaintenanceInstaller::Volta,
            MaintenanceTier::Volta,
            shim_unwrapped,
            install_path,
            "volta".to_owned(),
            Some(UpdateCommandSpec {
                executable: "volta".to_owned(),
                args: vec!["install".to_owned(), format!("{npm_pkg}@latest")],
            }),
            def.manual_hint.clone(),
        ));
    }
    if input.npm_global_prefix.is_some() || looks_like_node_modules_path(binary, target) {
        let prefix = derive_npm_prefix(binary, target, input.npm_global_prefix.as_deref());
        let mut args = vec!["install".to_owned(), "-g".to_owned()];
        let lock_key = if let Some(ref dir) = prefix {
            args.push("--prefix".to_owned());
            args.push(dir.clone());
            format!("npm-global:{dir}")
        } else {
            "npm-global".to_owned()
        };
        args.push(format!("{npm_pkg}@latest"));
        return Some(build_result(
            MaintenanceInstaller::NpmGlobal { prefix },
            MaintenanceTier::NpmGlobal,
            shim_unwrapped,
            install_path,
            lock_key,
            Some(UpdateCommandSpec {
                executable: "npm".to_owned(),
                args,
            }),
            def.manual_hint.clone(),
        ));
    }
    None
}

pub(super) fn resolve_js_package_manager(
    def: &ProviderMaintenanceDefinition,
    npm_pkg: &str,
    binary: Option<&str>,
    target: Option<&str>,
    input: &MaintenanceResolutionInput,
    shim_unwrapped: bool,
) -> Option<MaintenanceCapabilities> {
    resolve_vite_bun_pnpm(def, npm_pkg, binary, target, input, shim_unwrapped)
        .or_else(|| resolve_yarn_volta_npm(def, npm_pkg, binary, target, input, shim_unwrapped))
}

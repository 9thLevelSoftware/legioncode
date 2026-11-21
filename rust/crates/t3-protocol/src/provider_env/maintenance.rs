//! 15-tier provider maintenance resolver and exact-pin update policy.
//!
//! Fixes BUG-13 and BUG-14 in `packages/provider-core/src/server/maintenanceResolver.ts`:
//! - Grok (and all JS-packaged CLIs) evaluate Vite+/Bun/pnpm/Yarn/Volta/npm
//!   global package managers before falling back to native `update`.
//! - `bun-global` sets `supports_exact_pin = false` so `make_targeted_update_action`
//!   refuses exact `@<version>` pins that fail against Bun's `minimumReleaseAge`.

use super::maintenance_js::{
    build_result, is_nix_store_path, looks_like_any_js_manager_path, looks_like_node_modules_path,
    matches_js_store, resolve_js_package_manager,
};
use super::maintenance_types::{
    MaintenanceCapabilities, MaintenanceInstaller, MaintenanceProbe, MaintenanceResolutionInput,
    MaintenanceShimMode, MaintenanceTier, ProviderMaintenanceDefinition, UpdateCommandSpec,
};

fn unwrap_vite_plus_shim(
    input: &MaintenanceResolutionInput,
) -> (Option<String>, Option<String>, bool) {
    if input.vite_plus_shim_mode == MaintenanceShimMode::SystemFirst
        && let Some(unwrapped) = &input.vite_plus_unwrapped_path
        && matches_js_store(
            input.resolved_binary_path.as_deref(),
            input.resolved_target_path.as_deref(),
            &[".vite-plus"],
            &input.vite_plus_global_dirs,
        )
    {
        (
            Some(unwrapped.clone()),
            input.vite_plus_unwrapped_target_path.clone(),
            true,
        )
    } else {
        (
            input.resolved_binary_path.clone(),
            input.resolved_target_path.clone(),
            false,
        )
    }
}

fn resolve_windows_tiers(
    def: &ProviderMaintenanceDefinition,
    input: &MaintenanceResolutionInput,
    install_path: Option<&str>,
    shim_unwrapped: bool,
) -> Option<MaintenanceCapabilities> {
    if !input.is_windows {
        return None;
    }
    let owned_install = install_path.map(str::to_owned);
    if input.has_probe(MaintenanceProbe::WinGet)
        && let Some(winget_id) = &def.winget_package_id
    {
        return Some(build_result(
            MaintenanceInstaller::WinGet,
            MaintenanceTier::WinGet,
            shim_unwrapped,
            owned_install,
            format!("winget:{winget_id}"),
            Some(UpdateCommandSpec {
                executable: "winget".to_owned(),
                args: vec![
                    "upgrade".to_owned(),
                    "--id".to_owned(),
                    winget_id.clone(),
                    "--exact".to_owned(),
                    "--silent".to_owned(),
                ],
            }),
            def.manual_hint.clone(),
        ));
    }

    if input.has_probe(MaintenanceProbe::Chocolatey)
        && let Some(choco_pkg) = &def.choco_package_name
    {
        return Some(build_result(
            MaintenanceInstaller::Chocolatey,
            MaintenanceTier::Chocolatey,
            shim_unwrapped,
            owned_install,
            format!("choco:{choco_pkg}"),
            Some(UpdateCommandSpec {
                executable: "choco".to_owned(),
                args: vec!["upgrade".to_owned(), choco_pkg.clone(), "-y".to_owned()],
            }),
            def.manual_hint.clone(),
        ));
    }

    None
}

fn resolve_system_tiers(
    def: &ProviderMaintenanceDefinition,
    input: &MaintenanceResolutionInput,
    binary: Option<&str>,
    target: Option<&str>,
    install_path: Option<&str>,
    shim_unwrapped: bool,
) -> Option<MaintenanceCapabilities> {
    let owned_install = install_path.map(str::to_owned);

    if def.mise_upgrade_allowed
        && input.has_probe(MaintenanceProbe::MiseActiveLatest)
        && let Some(mise_pkg) = &def.mise_package
    {
        return Some(build_result(
            MaintenanceInstaller::Mise,
            MaintenanceTier::Mise,
            shim_unwrapped,
            owned_install,
            format!("mise:{mise_pkg}"),
            Some(UpdateCommandSpec {
                executable: "mise".to_owned(),
                args: vec!["upgrade".to_owned(), mise_pkg.clone()],
            }),
            def.manual_hint.clone(),
        ));
    }

    if is_nix_store_path(binary, target) {
        return Some(build_result(
            MaintenanceInstaller::Nix,
            MaintenanceTier::Nix,
            shim_unwrapped,
            owned_install,
            format!("nix:{}", def.provider),
            None,
            def.manual_hint.clone().or_else(|| {
                Some(format!(
                    "Update {} via your Nix flake or profile",
                    def.provider
                ))
            }),
        ));
    }

    if input.has_probe(MaintenanceProbe::MacPorts)
        && let Some(port_name) = &def.macports_port
    {
        return Some(build_result(
            MaintenanceInstaller::MacPorts,
            MaintenanceTier::MacPorts,
            shim_unwrapped,
            owned_install,
            format!("macports:{port_name}"),
            None,
            Some(format!("sudo port upgrade {port_name}")),
        ));
    }

    if let Some(native_args) = &def.native_update_args
        && input.has_probe(MaintenanceProbe::NativeBinaryPath)
        && !looks_like_any_js_manager_path(binary, target, input)
    {
        return Some(build_result(
            MaintenanceInstaller::NativeInstaller,
            MaintenanceTier::NativeInstaller,
            shim_unwrapped,
            owned_install.clone(),
            format!("native:{}", def.provider),
            Some(UpdateCommandSpec {
                executable: owned_install.unwrap_or_else(|| def.provider.clone()),
                args: native_args.clone(),
            }),
            def.manual_hint.clone(),
        ));
    }

    None
}

fn resolve_homebrew_tier(
    def: &ProviderMaintenanceDefinition,
    input: &MaintenanceResolutionInput,
    install_path: Option<&str>,
    shim_unwrapped: bool,
) -> Option<MaintenanceCapabilities> {
    if !input.has_probe(MaintenanceProbe::Homebrew) {
        return None;
    }
    let (name, is_cask) = if def.homebrew_cask_only {
        def.brew_cask.as_ref().map(|c| (c, true))?
    } else {
        def.brew_formula
            .as_ref()
            .map(|f| (f, false))
            .or_else(|| def.brew_cask.as_ref().map(|c| (c, true)))?
    };

    let mut args = vec!["upgrade".to_owned()];
    if is_cask {
        args.push("--cask".to_owned());
    }
    args.push(name.clone());
    Some(build_result(
        MaintenanceInstaller::Homebrew,
        MaintenanceTier::Homebrew,
        shim_unwrapped,
        install_path.map(str::to_owned),
        format!("homebrew:{name}"),
        Some(UpdateCommandSpec {
            executable: "brew".to_owned(),
            args,
        }),
        def.manual_hint.clone(),
    ))
}

/// Resolves provider update capabilities across the 15-tier maintenance precedence.
///
/// Evaluates Vite+ shim unwrapping, OS package managers (`WinGet`, Chocolatey, `mise`,
/// Nix, `MacPorts`), standalone native installer, JS global package managers (Vite+,
/// Bun, pnpm, Yarn, Volta, npm), Homebrew, and native/manual fallback in exact order.
pub fn resolve_provider_maintenance(
    def: &ProviderMaintenanceDefinition,
    input: &MaintenanceResolutionInput,
) -> MaintenanceCapabilities {
    let (binary_owned, target_owned, shim_unwrapped) = unwrap_vite_plus_shim(input);
    let binary = binary_owned.as_deref();
    let target = target_owned.as_deref();
    let install_path = binary.or(target);

    if let Some(early) = resolve_windows_tiers(def, input, install_path, shim_unwrapped)
        .or_else(|| resolve_system_tiers(def, input, binary, target, install_path, shim_unwrapped))
    {
        return early;
    }

    if let Some(npm_pkg) = &def.npm_package_name
        && let Some(js_caps) =
            resolve_js_package_manager(def, npm_pkg, binary, target, input, shim_unwrapped)
    {
        return js_caps;
    }

    if let Some(brew_caps) = resolve_homebrew_tier(def, input, install_path, shim_unwrapped) {
        return brew_caps;
    }

    if def.requires_command_path
        || looks_like_node_modules_path(binary, target)
        || def.native_update_args.is_none()
    {
        return build_result(
            MaintenanceInstaller::Manual,
            MaintenanceTier::Fallback,
            shim_unwrapped,
            install_path.map(str::to_owned),
            format!("manual:{}", def.provider),
            None,
            def.manual_hint.clone(),
        );
    }

    let native_args = def.native_update_args.clone().unwrap_or_default();
    build_result(
        MaintenanceInstaller::NativeFallback,
        MaintenanceTier::Fallback,
        shim_unwrapped,
        install_path.map(str::to_owned),
        format!("native:{}", def.provider),
        Some(UpdateCommandSpec {
            executable: install_path.map_or_else(|| def.provider.clone(), str::to_owned),
            args: native_args,
        }),
        def.manual_hint.clone(),
    )
}

/// Rewrites an update command to target an exact `version` when `supports_exact_pin` holds.
///
/// Returns `None` when pinning is unsupported (notably [`MaintenanceInstaller::BunGlobal`],
/// fixing BUG-14) or `version` is blank.
pub fn make_targeted_update_action(
    capabilities: &MaintenanceCapabilities,
    version: &str,
) -> Option<UpdateCommandSpec> {
    let clean_version = version.trim();
    if clean_version.is_empty() || !capabilities.supports_exact_pin {
        return None;
    }
    let cmd = capabilities.update_command.as_ref()?;
    let mut args = cmd.args.clone();
    let last = args.last_mut()?;
    if let Some(pkg) = last.strip_suffix("@latest") {
        *last = format!("{pkg}@{clean_version}");
    } else if !last.starts_with('-') {
        *last = format!("{last}@{clean_version}");
    }
    Some(UpdateCommandSpec {
        executable: cmd.executable.clone(),
        args,
    })
}

/// Chooses the effective update command for updating to the latest registry release.
///
/// Pins `@<latest_version>` exclusively for `pnpm-global` to bypass `minimumReleaseAge`
/// lag, while leaving `bun-global` (`bun i -g <pkg>@latest`) and other installers unpinned.
pub fn resolve_latest_update_action(
    capabilities: &MaintenanceCapabilities,
    latest_version: Option<&str>,
) -> Option<UpdateCommandSpec> {
    if capabilities.installer == MaintenanceInstaller::PnpmGlobal
        && let Some(version) = latest_version
        && let Some(targeted) = make_targeted_update_action(capabilities, version)
    {
        return Some(targeted);
    }
    capabilities.update_command.clone()
}
